//! A tiny text progress bar for downloads and jobs, drawn through the existing
//! carriage-return redraw in [`Out::progress`](crate::support::output::Out::progress).

/// A fixed-width bar for `fraction` (clamped to `0.0..=1.0`), e.g. `[####----]`.
/// `width` is the count of cells between the brackets.
pub fn bar(fraction: f64, width: usize) -> String {
    let fraction = fraction.clamp(0.0, 1.0);
    let filled = (fraction * width as f64).round() as usize;
    let filled = filled.min(width);
    format!("[{}{}]", "#".repeat(filled), "-".repeat(width - filled))
}

#[cfg(test)]
mod tests {
    use super::bar;

    #[test]
    fn empty_and_full_are_exact() {
        assert_eq!(bar(0.0, 8), "[--------]");
        assert_eq!(bar(1.0, 8), "[########]");
    }

    #[test]
    fn out_of_range_fractions_clamp() {
        assert_eq!(bar(-1.0, 4), "[----]");
        assert_eq!(bar(2.0, 4), "[####]");
    }

    #[test]
    fn half_rounds_to_half_width() {
        assert_eq!(bar(0.5, 10), "[#####-----]");
    }
}
