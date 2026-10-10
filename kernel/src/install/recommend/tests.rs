use super::*;
use crate::install::provider::InstallProviderId;
use crate::machine::testing::{apple_silicon, card, every_engine, linux, m5_pro};
use crate::machine::{
    Engines, FreeDisk, LLAMA_CPP_INSTALL_HINT, OLLAMA_INSTALL_HINT, Placement, UV_INSTALL_HINT,
};

/// A 16 GiB Mac, whose working set is two thirds of it.
fn small_mac() -> Machine {
    apple_silicon(16, 10923)
}

/// A Linux box with no card and `memory_gib` of memory, without llama.cpp.
fn cpu_only(memory_gib: u64) -> Machine {
    Machine {
        engines: Engines {
            llama_cpp: false,
            ..every_engine()
        },
        ..linux(memory_gib, Vec::new())
    }
}

fn run(machine: &Machine, installed: &[&str], all: bool) -> Vec<Recommendation> {
    let installed: HashSet<String> = installed.iter().map(ToString::to_string).collect();
    recommend(
        machine,
        &Ask {
            categories: &[],
            installed: &installed,
            all,
        },
    )
}

fn with_status(recs: &[Recommendation], status: Status) -> Vec<&str> {
    recs.iter()
        .filter(|rec| rec.status == status)
        .map(|rec| rec.entry.reference.as_str())
        .collect()
}

fn picks_in(recs: &[Recommendation], category: InstallCategory) -> Vec<&str> {
    recs.iter()
        .filter(|rec| rec.entry.category == category && rec.status == Status::Pick)
        .map(|rec| rec.entry.reference.as_str())
        .collect()
}

#[test]
fn a_big_mac_gets_the_largest_three_that_run_well_in_each_category() {
    let recs = run(&m5_pro(), &[], false);
    assert_eq!(
        picks_in(&recs, InstallCategory::Chat),
        ["qwen3.8:27b", "gemma4:26b", "gemma4:31b"]
    );
    assert_eq!(
        picks_in(&recs, InstallCategory::Code),
        ["qwen2.5-coder:14b", "qwen3.6:27b-coding", "qwen3-coder:30b"]
    );
    assert_eq!(
        picks_in(&recs, InstallCategory::Voice),
        ["mlx-community/Kokoro-82M-bf16"]
    );
    assert_eq!(
        picks_in(&recs, InstallCategory::Image),
        [
            "stabilityai/sdxl-turbo",
            "stabilityai/stable-diffusion-xl-base-1.0"
        ]
    );
    assert!(recs.iter().all(|rec| rec.status == Status::Pick));
    assert!(recs.iter().all(|rec| rec.notes.is_empty()));
}

#[test]
fn what_is_past_the_working_set_is_too_large_and_shown_only_when_asked() {
    let shown = run(&m5_pro(), &[], false);
    assert!(
        !shown
            .iter()
            .any(|rec| rec.entry.reference == "gpt-oss:120b")
    );
    let all = run(&m5_pro(), &[], true);
    let big = all
        .iter()
        .find(|rec| rec.entry.reference == "gpt-oss:120b")
        .unwrap();
    assert_eq!(big.status, Status::TooLarge);
    assert_eq!(big.fit.unwrap().assessment.verdict, FitVerdict::TooLarge);
}

#[test]
fn a_small_mac_is_judged_against_its_working_set() {
    let recs = run(&small_mac(), &[], true);
    assert_eq!(
        picks_in(&recs, InstallCategory::Chat),
        ["qwen3.5:4b", "gemma4:e4b", "gemma4:12b"]
    );
    // 14b fits only tightly in 10.7 GiB, so the one model that runs well is
    // the only pick.
    assert_eq!(picks_in(&recs, InstallCategory::Code), ["qwen2.5-coder:7b"]);
    assert!(picks_in(&recs, InstallCategory::Image).is_empty());
    assert_eq!(
        with_status(&recs, Status::TooLarge)
            .into_iter()
            .filter(|reference| reference.starts_with("stabilityai/"))
            .count(),
        2
    );
}

#[test]
fn what_is_on_the_shelf_takes_no_pick_and_is_listed() {
    let recs = run(&m5_pro(), &["gemma4:31b"], false);
    assert_eq!(
        picks_in(&recs, InstallCategory::Chat),
        ["gpt-oss:20b", "qwen3.8:27b", "gemma4:26b"]
    );
    assert_eq!(with_status(&recs, Status::OnShelf), ["gemma4:31b"]);
}

#[test]
fn with_nothing_running_well_the_smallest_that_fits_is_the_pick() {
    // 1.32 GB × 1.25 of 2 GiB is 77 %: a tight fit, the only one.
    let recs = run(&cpu_only(2), &[], false);
    let chat = picks_in(&recs, InstallCategory::Chat);
    assert_eq!(chat, ["qwen3.5:0.8b"]);
    let pick = recs
        .iter()
        .find(|rec| rec.entry.reference == "qwen3.5:0.8b")
        .unwrap();
    assert_eq!(pick.fit.unwrap().assessment.verdict, FitVerdict::TightFit);
}

#[test]
fn with_nothing_fitting_there_is_no_pick() {
    let recs = run(&cpu_only(1), &[], false);
    assert!(picks_in(&recs, InstallCategory::Chat).is_empty());
    let all = run(&cpu_only(1), &[], true);
    assert!(
        all.iter()
            .filter(|rec| rec.entry.category == InstallCategory::Chat)
            .all(|rec| rec.status == Status::TooLarge)
    );
}

#[test]
fn an_engine_that_cannot_run_here_is_never_a_pick() {
    let recs = run(&cpu_only(64), &[], true);
    let kokoro = recs
        .iter()
        .find(|rec| rec.entry.engine == Engine::Mlx)
        .unwrap();
    assert_eq!(kokoro.status, Status::NoEngine);
    assert!(kokoro.fit.is_none());
    assert!(kokoro.notes.is_empty());
    assert!(picks_in(&recs, InstallCategory::Voice).is_empty());
    assert!(
        !run(&cpu_only(64), &[], false)
            .iter()
            .any(|rec| rec.status == Status::NoEngine)
    );
}

#[test]
fn a_machine_with_no_engines_gets_the_same_picks_with_what_to_install() {
    let mut bare = m5_pro();
    bare.engines = Engines::default();
    let recs = run(&bare, &[], false);
    let equipped = run(&m5_pro(), &[], false);
    assert_eq!(
        with_status(&recs, Status::Pick),
        with_status(&equipped, Status::Pick)
    );
    for rec in &recs {
        let expected = match rec.entry.engine {
            Engine::Ollama => OLLAMA_INSTALL_HINT,
            Engine::LlamaCpp => LLAMA_CPP_INSTALL_HINT,
            Engine::Mlx | Engine::Torch => UV_INSTALL_HINT,
            Engine::Other => unreachable!(),
        };
        assert_eq!(
            rec.notes,
            [Note::Install {
                engine: rec.entry.engine,
                hint: expected
            }],
            "{}",
            rec.entry.reference
        );
    }
}

#[test]
fn a_download_past_the_free_disk_says_so() {
    let mut machine = m5_pro();
    machine.free_disk = vec![
        FreeDisk {
            provider: InstallProviderId::ollama(),
            bytes: 19_000_000_000,
        },
        FreeDisk {
            provider: InstallProviderId::huggingface(),
            bytes: 400_000_000_000,
        },
    ];
    let recs = run(&machine, &[], false);
    let note_of = |reference: &str| {
        recs.iter()
            .find(|rec| rec.entry.reference == reference)
            .unwrap()
            .notes
            .clone()
    };
    // 18.73 GB × 1.05 is past 19 GB; 17.74 GB × 1.05 is not.
    assert_eq!(
        note_of("gemma4:26b"),
        [Note::Disk {
            needs: (18_731_025_387_f64 * DISK_HEADROOM) as u64,
            free: 19_000_000_000
        }]
    );
    assert!(note_of("qwen3.8:27b").is_empty());
    // Hugging Face pulls land on the disk with room.
    assert!(note_of("stabilityai/stable-diffusion-xl-base-1.0").is_empty());
}

#[test]
fn a_card_too_small_for_a_model_runs_it_from_memory() {
    let machine = linux(64, vec![card("RTX 4070", 12)]);
    let recs = run(&machine, &[], true);
    // What the card cannot hold runs from 64 GiB of memory, so the largest
    // that run well there are the picks, each spilling.
    assert_eq!(
        picks_in(&recs, InstallCategory::Chat),
        ["qwen3.8:27b", "gemma4:26b", "gemma4:31b"]
    );
    let placement = |reference: &str| {
        recs.iter()
            .find(|rec| rec.entry.reference == reference)
            .and_then(|rec| rec.fit)
            .map(|fit| fit.placement)
    };
    assert_eq!(placement("gemma4:31b"), Some(Placement::Spill));
    assert_eq!(placement("gemma4:12b"), Some(Placement::Gpu));
}

#[test]
fn a_listed_status_is_a_pick_or_on_the_shelf() {
    assert!(Status::Pick.is_listed());
    assert!(Status::OnShelf.is_listed());
    for status in [Status::Fits, Status::TooLarge, Status::NoEngine] {
        assert!(!status.is_listed(), "{status:?}");
    }
}

#[test]
fn categories_narrow_the_list() {
    let installed = HashSet::new();
    let recs = recommend(
        &m5_pro(),
        &Ask {
            categories: &[InstallCategory::Code],
            installed: &installed,
            all: true,
        },
    );
    assert!(!recs.is_empty());
    assert!(
        recs.iter()
            .all(|rec| rec.entry.category == InstallCategory::Code)
    );
}

#[test]
fn asking_for_all_judges_every_entry_once() {
    let recs = run(&m5_pro(), &[], true);
    assert_eq!(recs.len(), entries().len());
    let mut ids: Vec<String> = recs.iter().map(|rec| rec.entry.id()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), entries().len());
}

#[test]
fn each_category_lists_smallest_first() {
    let recs = run(&m5_pro(), &[], true);
    for category in InstallCategory::ALL {
        let sizes: Vec<u64> = recs
            .iter()
            .filter(|rec| rec.entry.category == category)
            .map(|rec| rec.entry.serving_bytes)
            .collect();
        assert!(
            sizes.windows(2).all(|pair| pair[0] <= pair[1]),
            "{category:?}"
        );
    }
}
