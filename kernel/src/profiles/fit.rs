//! Whether a model fits in a machine's memory: a coarse runs-well / tight-fit /
//! too-large verdict from the model's footprint and the total RAM.

/// How well a model is expected to run given available memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FitVerdict {
    /// Comfortable — well under the runs-well fraction of memory.
    RunsWell,
    /// Fits, but with little headroom.
    TightFit,
    /// Won't fit comfortably.
    TooLarge,
}

/// A fit verdict plus the memory the model is estimated to require.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FitAssessment {
    /// The verdict.
    pub verdict: FitVerdict,
    /// The estimated required bytes (footprint × overhead).
    pub required_bytes: i64,
}

impl FitVerdict {
    /// Weights need working memory beyond the raw footprint.
    const MEMORY_OVERHEAD_FACTOR: f64 = 1.25;
    /// Below this share of memory a model runs well.
    const RUNS_WELL_FRACTION: f64 = 0.75;
    /// Below this share it's a tight fit; at or above it's too large.
    const TIGHT_FIT_FRACTION: f64 = 0.95;

    /// Assess a `footprint_mb` model against `total_memory_bytes`. Returns `None`
    /// when the footprint is unknown/non-positive or the memory total is zero.
    pub fn assess(footprint_mb: Option<i64>, total_memory_bytes: u64) -> Option<FitAssessment> {
        let footprint_mb = footprint_mb.filter(|&mb| mb > 0)?;
        if total_memory_bytes == 0 {
            return None;
        }
        let required_bytes =
            (footprint_mb as f64 * (1i64 << 20) as f64 * Self::MEMORY_OVERHEAD_FACTOR) as i64;
        let share = required_bytes as f64 / total_memory_bytes as f64;
        let verdict = if share < Self::RUNS_WELL_FRACTION {
            FitVerdict::RunsWell
        } else if share < Self::TIGHT_FIT_FRACTION {
            FitVerdict::TightFit
        } else {
            FitVerdict::TooLarge
        };
        Some(FitAssessment {
            verdict,
            required_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn an_unknown_or_empty_footprint_is_unassessable() {
        assert!(FitVerdict::assess(None, 16 * GIB).is_none());
        assert!(FitVerdict::assess(Some(0), 16 * GIB).is_none());
        assert!(FitVerdict::assess(Some(-5), 16 * GIB).is_none());
        assert!(FitVerdict::assess(Some(1000), 0).is_none());
    }

    #[test]
    fn the_verdict_tracks_the_memory_share() {
        // ~1 GiB footprint × 1.25 = ~1.25 GiB of 16 GiB → well under 0.75 → runs well.
        let assessment = FitVerdict::assess(Some(1024), 16 * GIB).unwrap();
        assert_eq!(assessment.verdict, FitVerdict::RunsWell);

        // 12 GiB × 1.25 = 15 GiB of 16 GiB → share 0.9375 → tight fit.
        let tight = FitVerdict::assess(Some(12 * 1024), 16 * GIB).unwrap();
        assert_eq!(tight.verdict, FitVerdict::TightFit);

        // 16 GiB × 1.25 = 20 GiB of 16 GiB → share 1.25 → too large.
        let too_large = FitVerdict::assess(Some(16 * 1024), 16 * GIB).unwrap();
        assert_eq!(too_large.verdict, FitVerdict::TooLarge);
    }

    #[test]
    fn required_bytes_includes_the_overhead_factor() {
        let assessment = FitVerdict::assess(Some(1024), 64 * GIB).unwrap();
        // 1024 MiB × 1.25 = 1280 MiB.
        assert_eq!(assessment.required_bytes, 1280 * (1 << 20));
    }
}
