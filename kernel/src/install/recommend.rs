//! What to recommend installing on a machine: the catalog judged against what
//! the machine can give a model, and against what is already on its shelf.
//!
//! The hardware decides what fits. Whether the engine that serves a model is
//! installed never changes that; it only adds a note saying what to install
//! first, so a machine with nothing on it yet still gets the picks its memory
//! earns.

use std::collections::HashSet;

use crate::install::catalog::{InstallCatalogEntry, InstallCategory, entries};
use crate::install::installed::is_installed;
use crate::install::plan::DISK_HEADROOM;
use crate::machine::{Engine, Fit, Machine, Readiness};
use crate::profiles::FitVerdict;

/// How many models each category recommends at most.
const PICKS_PER_CATEGORY: usize = 3;

/// What to recommend for.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    /// The categories to cover; every one when empty.
    pub categories: &'a [InstallCategory],
    /// The names already on the shelf, as
    /// [`installed_names`](crate::install::installed::installed_names) gives them.
    pub installed: &'a HashSet<String>,
    /// Every catalog entry, each with why it is or is not a pick; otherwise
    /// only the [listed](Status::is_listed) ones.
    pub all: bool,
}

/// Where a catalog entry stands on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    /// Recommended.
    Pick,
    /// Already on the shelf.
    OnShelf,
    /// Fits, but the category's picks are larger ones that also run well.
    Fits,
    /// Too large for what the machine can give it.
    TooLarge,
    /// Its engine cannot run on this machine at all.
    NoEngine,
}

impl Status {
    /// The stable string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Pick => "pick",
            Status::OnShelf => "on_shelf",
            Status::Fits => "fits",
            Status::TooLarge => "too_large",
            Status::NoEngine => "no_engine",
        }
    }

    /// Whether a list that is not asked for everything shows it: a pick, or
    /// what is already on the shelf.
    pub fn is_listed(self) -> bool {
        matches!(self, Status::Pick | Status::OnShelf)
    }
}

/// Something to know before pulling a recommendation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Note {
    /// The engine that serves it is not installed.
    Install {
        /// The engine.
        engine: Engine,
        /// How to install it.
        hint: &'static str,
    },
    /// The download is larger than the free disk, with headroom.
    Disk {
        /// The bytes the pull needs, headroom included.
        needs: u64,
        /// The bytes free.
        free: u64,
    },
}

/// One catalog entry judged for this machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Recommendation {
    /// The entry.
    pub entry: InstallCatalogEntry,
    /// Where it stands.
    pub status: Status,
    /// How it fits and where it would run; `None` when its engine cannot run
    /// here.
    pub fit: Option<Fit>,
    /// What to know before pulling it.
    pub notes: Vec<Note>,
}

/// The catalog judged for `machine`, category by category in the order
/// [`InstallCategory::ALL`] gives them and smallest first within each.
///
/// Each category picks the largest models that run well, up to three, from
/// those its engine can run here and that are not on the shelf. When none
/// runs well, it picks the smallest that still fits; when none fits, nothing.
pub fn recommend(machine: &Machine, ask: &Ask) -> Vec<Recommendation> {
    let mut catalog = entries();
    catalog.sort_by(|a, b| {
        a.serving_bytes
            .cmp(&b.serving_bytes)
            .then_with(|| a.reference.cmp(&b.reference))
    });
    InstallCategory::ALL
        .into_iter()
        .filter(|category| ask.categories.is_empty() || ask.categories.contains(category))
        .flat_map(|category| {
            let in_category: Vec<InstallCatalogEntry> = catalog
                .iter()
                .filter(|entry| entry.category == category)
                .cloned()
                .collect();
            judge_category(machine, ask, in_category)
        })
        .collect()
}

/// The recommendations of one category, from its `entries` smallest first.
fn judge_category(
    machine: &Machine,
    ask: &Ask,
    entries: Vec<InstallCatalogEntry>,
) -> Vec<Recommendation> {
    let mut judged: Vec<Recommendation> = entries
        .into_iter()
        .map(|entry| {
            let fit = entry.fit(machine);
            let status = match fit.map(|fit| fit.assessment.verdict) {
                None => Status::NoEngine,
                Some(_) if is_installed(&entry.reference, ask.installed) => Status::OnShelf,
                Some(FitVerdict::TooLarge) => Status::TooLarge,
                Some(_) => Status::Fits,
            };
            Recommendation {
                notes: notes(machine, &entry, status),
                entry,
                status,
                fit,
            }
        })
        .collect();

    let verdict = |rec: &Recommendation| rec.fit.map(|fit| fit.assessment.verdict);
    let running_well: Vec<usize> = (0..judged.len())
        .filter(|&i| judged[i].status == Status::Fits)
        .filter(|&i| verdict(&judged[i]) == Some(FitVerdict::RunsWell))
        .collect();
    let picks: Vec<usize> = if running_well.is_empty() {
        (0..judged.len())
            .find(|&i| judged[i].status == Status::Fits)
            .into_iter()
            .collect()
    } else {
        running_well
            .iter()
            .rev()
            .take(PICKS_PER_CATEGORY)
            .copied()
            .collect()
    };
    for index in picks {
        judged[index].status = Status::Pick;
    }
    if !ask.all {
        judged.retain(|rec| rec.status.is_listed());
    }
    judged
}

/// What to know before pulling `entry` with `status` on `machine`.
fn notes(machine: &Machine, entry: &InstallCatalogEntry, status: Status) -> Vec<Note> {
    let mut notes = Vec::new();
    if matches!(status, Status::NoEngine | Status::OnShelf) {
        return notes;
    }
    if let Readiness::Install(hint) = machine.readiness(entry.engine) {
        notes.push(Note::Install {
            engine: entry.engine,
            hint,
        });
    }
    let needs = (entry.download_bytes as f64 * DISK_HEADROOM) as u64;
    if let Some(free) = machine.free_disk_for(&entry.provider)
        && needs > free
    {
        notes.push(Note::Disk { needs, free });
    }
    notes
}

#[cfg(test)]
mod tests;
