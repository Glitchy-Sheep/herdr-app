//! Restart the current app after its normal shutdown has finished.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const RESUMING: &str = "HERDR_APP_SETTINGS_RESTART";
const READY: &str = "herdr-restart-ready";
static PENDING: AtomicBool = AtomicBool::new(false);

pub fn pending() -> bool {
    PENDING.load(Ordering::Acquire)
}

/// Called before app threads start. A settings restart leaves staged updates alone.
pub fn resuming() -> bool {
    let resuming = std::env::var_os(RESUMING).is_some_and(|value| value == "1");
    // SAFETY: main calls this before starting threads or connecting to the core.
    unsafe { std::env::remove_var(RESUMING) };
    resuming
}

/// Prepare a restart that waits for this process to exit. Errors leave the app running.
/// Only the caller receiving `true` should request shutdown.
pub fn prepare() -> Result<bool, String> {
    if PENDING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(false);
    }
    let result = std::env::current_exe()
        .map_err(|error| format!("Cannot find Herdr's executable: {error}"))
        .and_then(|exe| prepare_at(&exe, std::process::id()));
    if result.is_err() {
        PENDING.store(false, Ordering::Release);
    }
    result.map(|()| true)
}

fn prepare_at(exe: &Path, parent: u32) -> Result<(), String> {
    if !exe.is_file() {
        return Err(format!(
            "Herdr's executable is unavailable: {}",
            exe.display()
        ));
    }
    let ready =
        Readiness::new().map_err(|error| format!("Cannot prepare Herdr's restart: {error}"))?;
    let mut child = helper(exe, parent, &ready.file())
        .spawn()
        .map_err(|error| format!("Cannot start Herdr's restart helper: {error}"))?;
    if let Err(error) = wait_ready(&mut child, &ready.file()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(())
}

fn wait_ready(child: &mut Child, ready: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read_to_string(ready).is_ok_and(|text| text == READY) {
            return Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!(
                    "Herdr's restart helper exited before it was ready ({status})"
                ));
            }
            Err(error) => return Err(format!("Cannot check Herdr's restart helper: {error}")),
            Ok(None) => {}
        }
        if Instant::now() >= deadline {
            return Err("Herdr's restart helper did not become ready".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn helper(exe: &Path, parent: u32, ready: &Path) -> Command {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    let path = std::env::var_os("PATH").unwrap_or_default();
    let sleeper = std::env::split_paths(&path)
        .map(|directory| directory.join("sleep"))
        .chain([PathBuf::from("/bin/sleep"), PathBuf::from("/usr/bin/sleep")])
        .find(|path| {
            path.is_absolute()
                && std::fs::metadata(path)
                    .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0)
        })
        .unwrap_or_else(|| PathBuf::from("/bin/sleep"));
    let mut command = Command::new("/bin/sh");
    command
        .args([
            "-c",
            r#"
        test -f "$2" && test -x "$2" && test -x "$4" && kill -0 "$1" 2>/dev/null || exit 1
        printf '%s' 'herdr-restart-ready' > "$3" || exit 1
        while kill -0 "$1" 2>/dev/null; do "$4" 0.1; done
        exec "$2"
    "#,
            "herdr-restart",
        ])
        .arg(parent.to_string())
        .arg(exe)
        .arg(ready)
        .arg(sleeper)
        .env(RESUMING, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);
    command
}

#[cfg(windows)]
fn helper(exe: &Path, parent: u32, ready: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let powershell = std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe"))
        .unwrap_or_else(|| "powershell.exe".into());
    let mut command = Command::new(powershell);
    command.args(["-NoProfile", "-NonInteractive", "-Command", r#"
        $ErrorActionPreference = 'Stop'
        $exe = $env:HERDR_RESTART_EXECUTABLE
        $ready = $env:HERDR_RESTART_READY
        $parentProcess = Get-Process -Id ([int]$env:HERDR_RESTART_PARENT)
        $null = $parentProcess.Handle
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Herdr executable is unavailable' }
        $info = New-Object System.Diagnostics.ProcessStartInfo
        $info.FileName = $exe
        $info.UseShellExecute = $false
        $info.CreateNoWindow = $true
        [Environment]::SetEnvironmentVariable('HERDR_RESTART_EXECUTABLE', $null)
        [Environment]::SetEnvironmentVariable('HERDR_RESTART_READY', $null)
        [Environment]::SetEnvironmentVariable('HERDR_RESTART_PARENT', $null)
        [IO.File]::WriteAllText($ready, 'herdr-restart-ready')
        $parentProcess.WaitForExit()
        $null = [Diagnostics.Process]::Start($info)
    "#])
        .env("HERDR_RESTART_EXECUTABLE", exe).env("HERDR_RESTART_PARENT", parent.to_string())
        .env("HERDR_RESTART_READY", ready).env(RESUMING, "1")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .creation_flags(0x0800_0000); // CREATE_NO_WINDOW: the helper has no console.
    command
}

struct Readiness(PathBuf);

impl Readiness {
    fn new() -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        loop {
            let path = std::env::temp_dir().join(format!(
                "herdr-restart-{}-{time}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut directory = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                directory.mode(0o700);
            }
            match directory.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    fn file(&self) -> PathBuf {
        self.0.join("ready")
    }
}

impl Drop for Readiness {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.file());
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn helper_waits_for_exit_and_preserves_isolated_environment() {
        let directory = Readiness::new().unwrap();
        let exe = directory.0.join("Herdr space ' $() Юникод");
        let result = directory.0.join("result");
        std::fs::write(&exe, "#!/bin/sh\nprintf '%s\\n' \"$HERDR_APP_HOME\" \"$HC_SESSION\" \"$XDG_STATE_HOME\" \"$PATH\" \"$HERDR_APP_SETTINGS_RESTART\" > \"$HERDR_RESTART_TEST_OUTPUT\"\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut parent = Command::new("/bin/sh")
            .args(["-c", "read line"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut child = helper(&exe, parent.id(), &directory.file())
            .env("HERDR_APP_HOME", directory.0.join("isolated home"))
            .env("HC_SESSION", "test-herdr-settings-restart")
            .env("XDG_STATE_HOME", directory.0.join("core state"))
            .env("PATH", "/nonexistent")
            .env("HERDR_RESTART_TEST_OUTPUT", &result)
            .spawn()
            .unwrap();
        wait_ready(&mut child, &directory.file()).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            !result.exists(),
            "replacement must wait for the old app's complete exit"
        );
        drop(parent.stdin.take());
        parent.wait().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("restart helper did not exit");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let actual = std::fs::read_to_string(&result).unwrap();
        assert_eq!(
            actual,
            format!(
                "{}\ntest-herdr-settings-restart\n{}\n/nonexistent\n1\n",
                directory.0.join("isolated home").display(),
                directory.0.join("core state").display()
            )
        );
        std::fs::remove_file(exe).unwrap();
        std::fs::remove_file(result).unwrap();
    }

    #[test]
    fn missing_or_nonexecutable_targets_fail_before_quit() {
        let directory = Readiness::new().unwrap();
        let exe = directory.0.join("missing");
        assert!(prepare_at(&exe, std::process::id()).is_err());
        std::fs::write(&exe, "not executable").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(prepare_at(&exe, std::process::id()).is_err());
        std::fs::remove_file(exe).unwrap();
    }
}
