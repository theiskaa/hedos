//! Whether a model fits in the memory it may use: a coarse runs-well /
//! tight-fit / too-large verdict from the model's footprint and a budget on
//! one of two scales, and the count of each verdict across a shelf.

use crate::machine::Machine;
use crate::records::{ModelRecord, ModelState};

/// How well a model is expected to run given available memory, ordered from
/// best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FitVerdict {
    /// Comfortable — well under the runs-well fraction of memory.
    RunsWell,
    /// Fits, but with little headroom.
    TightFit,
    /// Won't fit comfortably.
    TooLarge,
}

/// What a memory figure measures, which decides how much of it a model may
/// take before it stops running well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scale {
    /// All of the machine's memory, shared with the system and every other
    /// program: a model runs well below 75 % of it and fits below 95 %.
    System,
    /// A figure that is already the share a model may use: the Metal working
    /// set, or a card's video memory. A model runs well below 90 % of it and
    /// fits up to all of it; taking 75 % again would count the system's share
    /// twice.
    Accelerator,
}

impl Scale {
    /// The runs-well and tight-fit shares of the budget.
    fn fractions(self) -> (f64, f64) {
        match self {
            Scale::System => (0.75, 0.95),
            Scale::Accelerator => (0.90, 1.0),
        }
    }
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
    /// The stable string form: `runs_well`, `tight_fit`, or `too_large`.
    pub fn as_str(&self) -> &'static str {
        match self {
            FitVerdict::RunsWell => "runs_well",
            FitVerdict::TightFit => "tight_fit",
            FitVerdict::TooLarge => "too_large",
        }
    }

    /// Weights need working memory beyond the raw footprint.
    const MEMORY_OVERHEAD_FACTOR: f64 = 1.25;

    /// Assess a model of `footprint_bytes` on disk against `budget_bytes`, a
    /// figure read on `scale`. Returns `None` when the footprint is unknown or
    /// non-positive, or the budget is zero.
    pub fn assess_in(
        footprint_bytes: Option<i64>,
        budget_bytes: u64,
        scale: Scale,
    ) -> Option<FitAssessment> {
        let footprint_bytes = footprint_bytes.filter(|&bytes| bytes > 0)?;
        if budget_bytes == 0 {
            return None;
        }
        let required_bytes = (footprint_bytes as f64 * Self::MEMORY_OVERHEAD_FACTOR) as i64;
        let share = required_bytes as f64 / budget_bytes as f64;
        let (runs_well, tight_fit) = scale.fractions();
        let verdict = if share < runs_well {
            FitVerdict::RunsWell
        } else if share < tight_fit {
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

/// How many models on a shelf come out at each verdict, judged by what
/// serving each loads on the engine it resolved to, as `hedos ls` and the
/// shelf judge them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FitTally {
    /// Models that run well.
    pub runs_well: usize,
    /// Models that fit with little headroom.
    pub tight_fit: usize,
    /// Models too large for the memory.
    pub too_large: usize,
    /// Models with no serving size to judge.
    pub unknown: usize,
}

impl FitTally {
    /// Tally `records` on `machine`. A model whose weights are gone is left
    /// out: it has no verdict, only that it is gone.
    pub fn over<'a>(records: impl IntoIterator<Item = &'a ModelRecord>, machine: &Machine) -> Self {
        let mut tally = Self::default();
        for record in records {
            if record.state == ModelState::Missing {
                continue;
            }
            let verdict = machine.fit_record(record).map(|fit| fit.assessment.verdict);
            match verdict {
                Some(FitVerdict::RunsWell) => tally.runs_well += 1,
                Some(FitVerdict::TightFit) => tally.tight_fit += 1,
                Some(FitVerdict::TooLarge) => tally.too_large += 1,
                None => tally.unknown += 1,
            }
        }
        tally
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{Modality, ModelSource, SourceKind};

    const GIB: i64 = 1 << 30;
    const MEMORY: u64 = 16 * (1 << 30);

    #[test]
    fn an_unknown_or_empty_footprint_is_unassessable() {
        assert!(FitVerdict::assess_in(None, MEMORY, Scale::System).is_none());
        assert!(FitVerdict::assess_in(Some(0), MEMORY, Scale::System).is_none());
        assert!(FitVerdict::assess_in(Some(-5), MEMORY, Scale::System).is_none());
        assert!(FitVerdict::assess_in(Some(GIB), 0, Scale::System).is_none());
    }

    #[test]
    fn the_verdict_tracks_the_memory_share() {
        // 1 GiB footprint × 1.25 = 1.25 GiB of 16 GiB → well under 0.75 → runs well.
        let assessment = FitVerdict::assess_in(Some(GIB), MEMORY, Scale::System).unwrap();
        assert_eq!(assessment.verdict, FitVerdict::RunsWell);

        // 12 GiB × 1.25 = 15 GiB of 16 GiB → share 0.9375 → tight fit.
        let tight = FitVerdict::assess_in(Some(12 * GIB), MEMORY, Scale::System).unwrap();
        assert_eq!(tight.verdict, FitVerdict::TightFit);

        // 16 GiB × 1.25 = 20 GiB of 16 GiB → share 1.25 → too large.
        let too_large = FitVerdict::assess_in(Some(16 * GIB), MEMORY, Scale::System).unwrap();
        assert_eq!(too_large.verdict, FitVerdict::TooLarge);
    }

    #[test]
    fn an_accelerator_budget_is_already_the_usable_share() {
        // 8 GiB × 1.25 = 10 GiB. Of a 16 GiB system that is 0.625, runs well;
        // of a 10.5 GiB accelerator it is 0.95, which fits only tightly.
        let budget = 21 * GIB as u64 / 2;
        let system = FitVerdict::assess_in(Some(8 * GIB), MEMORY, Scale::System).unwrap();
        assert_eq!(system.verdict, FitVerdict::RunsWell);
        let tight = FitVerdict::assess_in(Some(8 * GIB), budget, Scale::Accelerator).unwrap();
        assert_eq!(tight.verdict, FitVerdict::TightFit);

        // 0.89 of the accelerator runs well, where on the system scale it
        // would only be tight.
        let snug =
            FitVerdict::assess_in(Some(8 * GIB), 1124 * GIB as u64 / 100, Scale::Accelerator)
                .unwrap();
        assert_eq!(snug.verdict, FitVerdict::RunsWell);

        // All of the accelerator and beyond is too large.
        let over =
            FitVerdict::assess_in(Some(8 * GIB), 10 * GIB as u64, Scale::Accelerator).unwrap();
        assert_eq!(over.verdict, FitVerdict::TooLarge);
        assert!(FitVerdict::assess_in(Some(GIB), 0, Scale::Accelerator).is_none());
    }

    #[test]
    fn each_verdict_has_a_stable_slug() {
        assert_eq!(FitVerdict::RunsWell.as_str(), "runs_well");
        assert_eq!(FitVerdict::TightFit.as_str(), "tight_fit");
        assert_eq!(FitVerdict::TooLarge.as_str(), "too_large");
    }

    fn sized(name: &str, footprint_bytes: Option<i64>) -> ModelRecord {
        let mut record = ModelRecord::new(
            name,
            Modality::text(),
            Vec::new(),
            ModelSource::new(SourceKind::file(), name),
        );
        record.footprint_bytes = footprint_bytes;
        record
    }

    #[test]
    fn a_tally_counts_each_verdict() {
        let records = [
            sized("small", Some(GIB)),
            sized("tight", Some(12 * GIB)),
            sized("huge", Some(16 * GIB)),
            sized("unsized", None),
        ];
        let tally = FitTally::over(&records, &Machine::with_memory(MEMORY));
        assert_eq!(
            tally,
            FitTally {
                runs_well: 1,
                tight_fit: 1,
                too_large: 1,
                unknown: 1,
            }
        );
    }

    #[test]
    fn a_tally_judges_what_serving_loads_not_the_disk() {
        let mut repo = sized("repo", Some(40 * GIB));
        repo.serving_bytes = Some(GIB);
        assert_eq!(
            FitTally::over([&repo], &Machine::with_memory(MEMORY)).runs_well,
            1
        );
    }

    #[test]
    fn a_tally_skips_a_model_whose_weights_are_gone() {
        let mut gone = sized("gone", Some(16 * GIB));
        gone.state = ModelState::Missing;
        let mut unsized_gone = sized("unsized-gone", None);
        unsized_gone.state = ModelState::Missing;
        assert_eq!(
            FitTally::over([&gone, &unsized_gone], &Machine::with_memory(MEMORY)),
            FitTally::default()
        );
    }

    #[test]
    fn a_tally_over_nothing_is_empty() {
        assert_eq!(
            FitTally::over(&[], &Machine::with_memory(MEMORY)),
            FitTally::default()
        );
    }

    #[test]
    fn required_bytes_includes_the_overhead_factor() {
        let assessment = FitVerdict::assess_in(Some(GIB), 64 * (1 << 30), Scale::System).unwrap();
        // 1 GiB × 1.25 = 1.25 GiB.
        assert_eq!(assessment.required_bytes, 1280 * (1 << 20));
    }
}
