//! Pointer-relative resizing for a centered, top-attached quick terminal.

#[derive(Clone, Copy)]
struct Drag {
    frame: [f64; 4],
    pointer: [f64; 2],
    horizontal: f64,
    bottom: bool,
}

pub(crate) struct QuickResize {
    bounds: [f64; 4],
    drag: Option<Drag>,
    pub(crate) samples: u64,
}

impl QuickResize {
    pub(crate) fn new(bounds: [f64; 4]) -> Self {
        Self {
            bounds,
            drag: None,
            samples: 0,
        }
    }

    pub(crate) fn begin(&mut self, frame: [f64; 4], pointer: [f64; 2], bounds: [f64; 4]) {
        self.bounds = bounds;
        let horizontal = if (pointer[0] - frame[0]).abs() <= 12.0 {
            -1.0
        } else if (pointer[0] - frame[0] - frame[2]).abs() <= 12.0 {
            1.0
        } else {
            0.0
        };
        self.drag = Some(Drag {
            frame,
            pointer,
            horizontal,
            bottom: (pointer[1] - frame[1]).abs() <= 12.0,
        });
    }

    pub(crate) fn end(&mut self) {
        self.drag = None;
    }

    pub(crate) fn constrain(
        &mut self,
        proposed: [f64; 4],
        pointer: [f64; 2],
        minimum: [f64; 2],
    ) -> [f64; 4] {
        let Some(drag) = self.drag else {
            return proposed;
        };
        self.samples = self.samples.saturating_add(1);
        let horizontal = if drag.horizontal == 0.0 && proposed[2] != drag.frame[2] {
            if drag.pointer[0] < drag.frame[0] + drag.frame[2] / 2.0 {
                -1.0
            } else {
                1.0
            }
        } else {
            drag.horizontal
        };
        let bottom = drag.bottom
            || (proposed[3] != drag.frame[3]
                && drag.pointer[1] < drag.frame[1] + drag.frame[3] / 2.0);
        let width = drag.frame[2] + horizontal * 2.0 * (pointer[0] - drag.pointer[0]);
        let height = drag.frame[3]
            - if bottom {
                pointer[1] - drag.pointer[1]
            } else {
                0.0
            };
        let max_width = self.bounds[2].max(1.0);
        let max_height = self.bounds[3].max(1.0);
        let width = width
            .round()
            .clamp(minimum[0].max(1.0).min(max_width), max_width);
        let height = height
            .round()
            .clamp(minimum[1].max(1.0).min(max_height), max_height);
        [
            self.bounds[0] + (self.bounds[2] - width) / 2.0,
            self.bounds[1] + self.bounds[3] - height,
            width,
            height,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: [f64; 4] = [-1920.0, 40.0, 1920.0, 1040.0];
    const FRAME: [f64; 4] = [-1560.0, 480.0, 1200.0, 600.0];

    #[test]
    fn right_edge_tracks_the_pointer_and_left_edge_mirrors_it() {
        let mut resize = QuickResize::new(SCREEN);
        resize.begin(FRAME, [-360.0, 800.0], SCREEN);
        let grown = resize.constrain(
            [-1560.0, 480.0, 1300.0, 600.0],
            [-260.0, 800.0],
            [200.0, 100.0],
        );
        assert_eq!(grown, [-1660.0, 480.0, 1400.0, 600.0]);
        assert_eq!(grown[0] + grown[2], -260.0);
        assert_eq!(
            resize.constrain(grown, [-260.0, 800.0], [200.0, 100.0]),
            grown
        );
        let shrunk = resize.constrain(grown, [-460.0, 800.0], [200.0, 100.0]);
        assert_eq!(shrunk, [-1460.0, 480.0, 1000.0, 600.0]);
    }

    #[test]
    fn left_bottom_corner_keeps_the_center_and_top_fixed() {
        let mut resize = QuickResize::new(SCREEN);
        resize.begin(FRAME, [-1560.0, 480.0], SCREEN);
        let frame = resize.constrain(FRAME, [-1660.0, 380.0], [200.0, 100.0]);
        assert_eq!(frame, [-1660.0, 380.0, 1400.0, 700.0]);
        assert_eq!(frame[0] + frame[2] / 2.0, -960.0);
        assert_eq!(frame[1] + frame[3], 1080.0);
    }

    #[test]
    fn clamps_to_the_display_and_window_minimum_without_pointer_drift() {
        let mut resize = QuickResize::new(SCREEN);
        resize.begin(FRAME, [-360.0, 480.0], SCREEN);
        assert_eq!(
            resize.constrain(FRAME, [600.0, -900.0], [200.0, 100.0]),
            SCREEN
        );
        assert_eq!(
            resize.constrain(SCREEN, [-950.0, 1070.0], [200.0, 100.0]),
            [-1060.0, 980.0, 200.0, 100.0]
        );
    }

    #[test]
    fn the_top_edge_cannot_move_and_programmatic_frames_are_unchanged() {
        let mut resize = QuickResize::new(SCREEN);
        assert_eq!(resize.constrain(FRAME, [0.0, 0.0], [200.0, 100.0]), FRAME);
        resize.begin(FRAME, [-960.0, 1080.0], SCREEN);
        assert_eq!(
            resize.constrain(FRAME, [-960.0, 1000.0], [200.0, 100.0]),
            FRAME
        );
        resize.end();
        assert_eq!(resize.constrain(SCREEN, [0.0, 0.0], [200.0, 100.0]), SCREEN);
    }

    #[test]
    fn each_drag_refreshes_the_workarea_without_switching_it_mid_drag() {
        let mut resize = QuickResize::new(SCREEN);
        let current = [-1920.0, 80.0, 1920.0, 960.0];
        let frame = [-1560.0, 440.0, 1200.0, 600.0];
        resize.begin(frame, [-360.0, 440.0], current);
        assert_eq!(
            resize.constrain(frame, [1000.0, -1000.0], [200.0, 100.0]),
            current
        );
    }
}
