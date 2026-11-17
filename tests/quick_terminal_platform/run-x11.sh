#!/usr/bin/env bash
set -euo pipefail

harness_dir=$(cd "$(dirname "$0")" && pwd)
case "${1:-}" in
  --host)
    # xvfb-run starts a separate display with its own Xauthority cookie.
    exec xvfb-run -a -s "-screen 0 1280x800x24 -nolisten tcp" bash -eu -c '
      openbox >/dev/null 2>&1 &
      wm=$!
      trap "kill $wm 2>/dev/null || true" EXIT
      export HERDR_QUICK_TEST_ISOLATED=1
      cargo test --locked --manifest-path "$1/Cargo.toml"
      cargo run --locked --manifest-path "$1/Cargo.toml"
    ' bash "$harness_dir"
    ;;
  --container)
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq >/tmp/apt-update.log
    apt-get install -y --no-install-recommends xvfb openbox xauth x11-xkb-utils >/tmp/apt-install.log 2>&1
    work=/work/tests/quick_terminal_platform
    mkdir -p "$work"
    cp "$harness_dir"/{Cargo.toml,Cargo.lock,run-x11.sh} "$work/"
    cp -r "$harness_dir/src" "$work/"
    exec bash "$work/run-x11.sh" --host
    ;;
  "")
    repo_dir=$(cd "$harness_dir/../.." && pwd)
    exec docker run --rm --init \
      --mount "type=bind,source=$harness_dir,target=/harness,readonly" \
      --mount "type=bind,source=$repo_dir/src/quick_terminal,target=/work/src/quick_terminal,readonly" \
      rust:1.95-bookworm bash /harness/run-x11.sh --container
    ;;
  *) echo "Usage: $0 [--host]" >&2; exit 2 ;;
esac
