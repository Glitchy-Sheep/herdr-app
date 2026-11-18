/// A tap finishes on release. The gap runs from the first release to the next press.
pub(super) const MAX_PRESS_MS: u32 = 300;
pub(super) const MAX_GAP_MS: u32 = 400;

#[cfg(test)]
pub(super) fn check_tap_pairs(recognizes: impl Fn([u32; 4]) -> bool) {
    for (edges, expected) in [
        ([0, 50, 350, 500], true),
        ([0, 300, 700, 1000], true),
        ([0, 301, 350, 400], false),
        ([0, 50, 451, 500], false),
        ([0, 50, 350, 651], false),
    ] {
        assert_eq!(recognizes(edges), expected, "tap edges in ms: {edges:?}");
    }
}
