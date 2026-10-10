use super::testing::{GIB, apple_silicon, card, every_engine, linux, m5_pro};
use super::*;
use crate::records::{Modality, ModelSource, SourceKind};

fn intel_mac() -> Machine {
    Machine {
        os: Os::Macos,
        arch: "x86_64".to_owned(),
        ..Machine::with_memory(32 * GIB)
    }
}

fn gib(n: u64) -> Option<i64> {
    Some((n * GIB) as i64)
}

fn verdict(machine: &Machine, engine: Engine, size: Option<i64>) -> Option<FitVerdict> {
    machine.fit(engine, size).map(|fit| fit.assessment.verdict)
}

#[test]
fn apple_silicon_budgets_the_metal_engines_against_the_working_set() {
    let machine = m5_pro();
    for engine in [Engine::Ollama, Engine::LlamaCpp, Engine::Mlx] {
        let budget = machine.budget(engine).unwrap();
        assert_eq!(budget.bytes, 53084 << 20, "{engine:?}");
        assert_eq!(budget.scale, Scale::Accelerator);
        assert_eq!(budget.placement, Placement::Gpu);
        assert!(!budget.spills);
    }
    assert_eq!(machine.models_budget_bytes(), 53084 << 20);
}

#[test]
fn apple_silicon_budgets_torch_against_all_of_memory() {
    let budget = m5_pro().budget(Engine::Torch).unwrap();
    assert_eq!(budget.bytes, 64 * GIB);
    assert_eq!(budget.scale, Scale::System);
    assert_eq!(budget.placement, Placement::Gpu);
}

#[test]
fn apple_silicon_without_a_metal_reading_falls_back_to_memory_on_the_gpu() {
    let machine = Machine {
        devices: Vec::new(),
        devices_from: DevicesFrom::None,
        ..m5_pro()
    };
    let budget = machine.budget(Engine::Ollama).unwrap();
    assert_eq!(budget.bytes, 64 * GIB);
    assert_eq!(budget.scale, Scale::System);
    assert_eq!(budget.placement, Placement::Gpu);
}

#[test]
fn mlx_runs_only_on_apple_silicon() {
    assert!(m5_pro().budget(Engine::Mlx).is_some());
    assert!(intel_mac().budget(Engine::Mlx).is_none());
    assert!(
        linux(64, vec![card("RTX 4090", 24)])
            .budget(Engine::Mlx)
            .is_none()
    );
    assert!(Machine::with_memory(16 * GIB).budget(Engine::Mlx).is_none());
    assert!(is_apple_silicon(Os::Macos, "aarch64"));
    assert!(!is_apple_silicon(Os::Linux, "aarch64"));
}

#[test]
fn an_intel_mac_runs_everything_else_on_the_processor() {
    let machine = intel_mac();
    for engine in [Engine::Ollama, Engine::LlamaCpp, Engine::Torch] {
        let budget = machine.budget(engine).unwrap();
        assert_eq!(budget.bytes, 32 * GIB);
        assert_eq!(budget.scale, Scale::System);
        assert_eq!(budget.placement, Placement::Cpu);
    }
}

#[test]
fn one_card_holds_what_it_can_and_the_rest_runs_from_memory() {
    let machine = linux(64, vec![card("RTX 4070", 12)]);
    let budget = machine.budget(Engine::Ollama).unwrap();
    assert_eq!(budget.bytes, 12 * GIB);
    assert_eq!(budget.scale, Scale::Accelerator);
    assert_eq!(budget.placement, Placement::Gpu);
    assert!(budget.spills);

    // 8 × 1.25 = 10 GiB of 12: on the card.
    let small = machine.fit(Engine::Ollama, gib(8)).unwrap();
    assert_eq!(small.assessment.verdict, FitVerdict::RunsWell);
    assert_eq!(small.placement, Placement::Gpu);
    assert_eq!(small.budget_bytes, 12 * GIB);

    // 20 × 1.25 = 25 GiB: past the card, 39 % of memory.
    let spilled = machine.fit(Engine::Ollama, gib(20)).unwrap();
    assert_eq!(spilled.assessment.verdict, FitVerdict::RunsWell);
    assert_eq!(spilled.placement, Placement::Spill);
    assert_eq!(spilled.budget_bytes, 64 * GIB);

    // 50 × 1.25 = 62.5 GiB: too big for the card and for memory, and said
    // against memory, the larger ceiling.
    let huge = machine.fit(Engine::Ollama, gib(50)).unwrap();
    assert_eq!(huge.assessment.verdict, FitVerdict::TooLarge);
    assert_eq!(huge.budget_bytes, 64 * GIB);
}

#[test]
fn a_model_that_fits_the_card_tightly_stays_on_it_unless_memory_reads_better() {
    // 9 × 1.25 = 11.25 GiB: 94 % of the card, tight; 18 % of memory, runs
    // well from there.
    let machine = linux(64, vec![card("RTX 4070", 12)]);
    let fit = machine.fit(Engine::Ollama, gib(9)).unwrap();
    assert_eq!(fit.assessment.verdict, FitVerdict::RunsWell);
    assert_eq!(fit.placement, Placement::Spill);
    // With only 12 GiB of memory too, memory reads no better: tight on the card.
    let lean = linux(12, vec![card("RTX 4070", 12)]);
    let fit = lean.fit(Engine::Ollama, gib(9)).unwrap();
    assert_eq!(fit.assessment.verdict, FitVerdict::TightFit);
    assert_eq!(fit.placement, Placement::Gpu);
}

#[test]
fn a_small_card_never_judges_a_model_worse_than_the_processor_alone() {
    let with_card = linux(32, vec![card("GTX 1650", 4)]);
    let without = linux(32, Vec::new());
    for size in 1..=40 {
        assert!(
            verdict(&with_card, Engine::Ollama, gib(size))
                <= verdict(&without, Engine::Ollama, gib(size)),
            "{size} GiB"
        );
    }
}

#[test]
fn llama_cpp_splits_across_cards_and_torch_takes_the_largest() {
    let machine = linux(64, vec![card("RTX 3090", 24), card("RTX 3060", 12)]);
    assert_eq!(machine.budget(Engine::LlamaCpp).unwrap().bytes, 36 * GIB);
    let torch = machine.budget(Engine::Torch).unwrap();
    assert_eq!(torch.bytes, 24 * GIB);
    assert!(!torch.spills);
}

#[test]
fn a_card_too_small_to_hold_a_model_is_left_out() {
    let carve_out = Device {
        name: "AMD Radeon Graphics".to_owned(),
        kind: DeviceKind::Discrete,
        memory_bytes: 512 << 20,
    };
    assert!(!carve_out.usable());
    let machine = linux(64, vec![carve_out]);
    let budget = machine.budget(Engine::Ollama).unwrap();
    assert_eq!(budget.bytes, 64 * GIB);
    assert_eq!(budget.placement, Placement::Cpu);
}

#[test]
fn a_machine_known_only_by_its_memory_judges_on_the_system_scale() {
    let machine = Machine::with_memory(16 * GIB);
    for footprint in [GIB, 12 * GIB, 16 * GIB] {
        let size = Some(footprint as i64);
        assert_eq!(
            machine.fit(Engine::Ollama, size).map(|fit| fit.assessment),
            FitVerdict::assess_in(size, 16 * GIB, Scale::System)
        );
    }
    assert_eq!(
        machine.fit(Engine::Ollama, gib(1)).unwrap().placement,
        Placement::Cpu
    );
}

#[test]
fn the_working_set_is_stricter_than_a_share_of_all_memory() {
    // 40 × 1.25 = 50 GiB: under 95 % of 64 GiB, but past 90 % of 51.8 GiB.
    let machine = m5_pro();
    let fit = machine.fit(Engine::Ollama, gib(40)).unwrap();
    assert_eq!(fit.assessment.verdict, FitVerdict::TightFit);
    assert_eq!(fit.budget_bytes, 53084 << 20);
    // 42 × 1.25 = 52.5 GiB: past the working set, too large on the GPU.
    assert_eq!(
        verdict(&machine, Engine::Ollama, gib(42)),
        Some(FitVerdict::TooLarge)
    );
}

#[test]
fn an_unknown_size_has_no_fit() {
    assert!(m5_pro().fit(Engine::Ollama, None).is_none());
}

#[test]
fn fit_on_falls_back_to_memory_where_the_engine_cannot_run() {
    let machine = intel_mac();
    assert!(machine.fit(Engine::Mlx, gib(1)).is_none());
    let fit = machine.fit_on(Engine::Mlx, gib(1)).unwrap();
    assert_eq!(fit.assessment.verdict, FitVerdict::RunsWell);
    assert_eq!(fit.budget_bytes, 32 * GIB);
}

#[test]
fn runtimes_map_to_their_engines() {
    assert_eq!(Engine::of_runtime(&RuntimeId::ollama()), Engine::Ollama);
    assert_eq!(
        Engine::of_runtime(&RuntimeId::llama_cpp()),
        Engine::LlamaCpp
    );
    assert_eq!(
        Engine::of_runtime(&RuntimeId::whisper_cpp()),
        Engine::LlamaCpp
    );
    for mlx in [
        RuntimeId::mlx_lm(),
        RuntimeId::mlx_vlm(),
        RuntimeId::mlx_audio(),
        RuntimeId::mflux(),
        RuntimeId::mlx_swift(),
    ] {
        assert_eq!(Engine::of_runtime(&mlx), Engine::Mlx);
    }
    assert_eq!(Engine::of_runtime(&RuntimeId::diffusers()), Engine::Torch);
    assert_eq!(Engine::of_runtime(&RuntimeId::embeddings()), Engine::Torch);
    assert_eq!(
        Engine::of_runtime(&RuntimeId::apple_foundation()),
        Engine::Other
    );
    assert_eq!(
        Engine::of_runtime(&RuntimeId::from("cli:tessera")),
        Engine::Other
    );
}

fn record(runtime: Option<RuntimeId>, footprint: i64) -> ModelRecord {
    let mut record = ModelRecord::new(
        "m",
        Modality::text(),
        Vec::new(),
        ModelSource::new(SourceKind::file(), "m"),
    );
    record.footprint_bytes = Some(footprint);
    record.runtime.id = runtime;
    record
}

#[test]
fn a_record_is_judged_on_its_engine() {
    let machine = m5_pro();
    // 42 GiB on Ollama is past the working set; unresolved, it is judged on
    // all of memory, where 52.5 of 64 GiB is tight.
    let size = (42 * GIB) as i64;
    let on_ollama = machine.fit_record(&record(Some(RuntimeId::ollama()), size));
    assert_eq!(on_ollama.unwrap().assessment.verdict, FitVerdict::TooLarge);
    let unresolved = machine.fit_record(&record(None, size)).unwrap();
    assert_eq!(unresolved.assessment.verdict, FitVerdict::TightFit);
    assert_eq!(unresolved.budget_bytes, 64 * GIB);
}

#[test]
fn a_record_on_an_engine_that_cannot_run_here_falls_back_to_memory() {
    let machine = intel_mac();
    let fit = machine.fit_record(&record(Some(RuntimeId::mlx_lm()), GIB as i64));
    assert_eq!(fit.unwrap().assessment.verdict, FitVerdict::RunsWell);
}

#[test]
fn free_disk_is_read_per_provider() {
    let mut machine = m5_pro();
    machine.free_disk = vec![FreeDisk {
        provider: InstallProviderId::ollama(),
        bytes: 5,
    }];
    assert_eq!(machine.free_disk_for(&InstallProviderId::ollama()), Some(5));
    assert_eq!(
        machine.free_disk_for(&InstallProviderId::huggingface()),
        None
    );
}

#[test]
fn readiness_names_what_to_install() {
    let mut machine = apple_silicon(64, 53084);
    machine.engines = Engines::default();
    assert_eq!(
        machine.readiness(Engine::Ollama),
        Readiness::Install(OLLAMA_INSTALL_HINT)
    );
    assert_eq!(
        machine.readiness(Engine::LlamaCpp),
        Readiness::Install(LLAMA_CPP_INSTALL_HINT)
    );
    assert_eq!(
        machine.readiness(Engine::Mlx),
        Readiness::Install(UV_INSTALL_HINT)
    );
    assert_eq!(
        machine.readiness(Engine::Torch),
        Readiness::Install(UV_INSTALL_HINT)
    );
    assert_eq!(machine.readiness(Engine::Other), Readiness::Ready);
    assert_eq!(Engine::Other.label(), None);

    machine.engines = every_engine();
    for engine in [Engine::Ollama, Engine::LlamaCpp, Engine::Mlx, Engine::Torch] {
        assert_eq!(machine.readiness(engine), Readiness::Ready);
    }
}
