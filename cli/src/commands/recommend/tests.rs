use std::collections::HashSet;

use kernel::install::InstallProviderId;
use kernel::machine::{
    Cores, Device, DeviceKind, DevicesFrom, Engines, FreeDisk, OLLAMA_INSTALL_HINT, Os,
    UV_INSTALL_HINT,
};

use super::*;

const GIB: u64 = 1 << 30;

fn every_engine() -> Engines {
    Engines {
        ollama: true,
        llama_cpp: true,
        uv: true,
    }
}

/// A 64 GiB M5 Pro, whose Metal working set is 53084 MiB.
fn big_mac() -> Machine {
    Machine {
        os: Os::Macos,
        arch: "aarch64".to_owned(),
        chip: Some("Apple M5 Pro".to_owned()),
        cores: Cores {
            performance: Some(6),
            efficiency: Some(12),
            logical: 18,
        },
        devices: vec![Device {
            name: "Apple M5 Pro".to_owned(),
            kind: DeviceKind::Unified,
            memory_bytes: 53084 << 20,
        }],
        devices_from: DevicesFrom::Metal,
        engines: every_engine(),
        free_disk: free_disk(412, 412),
        ..Machine::with_memory(64 * GIB)
    }
}

fn free_disk(ollama_gib: u64, hugging_face_gib: u64) -> Vec<FreeDisk> {
    vec![
        FreeDisk {
            provider: InstallProviderId::ollama(),
            bytes: ollama_gib * GIB,
        },
        FreeDisk {
            provider: InstallProviderId::huggingface(),
            bytes: hugging_face_gib * GIB,
        },
    ]
}

fn small_linux_box() -> Machine {
    Machine {
        os: Os::Linux,
        arch: "x86_64".to_owned(),
        cores: Cores {
            performance: None,
            efficiency: None,
            logical: 8,
        },
        engines: every_engine(),
        ..Machine::with_memory(8 * GIB)
    }
}

fn judge(machine: &Machine, installed: &[&str]) -> Vec<Recommendation> {
    let installed: HashSet<String> = installed.iter().map(ToString::to_string).collect();
    recommend(
        machine,
        &Ask {
            categories: &[],
            installed: &installed,
            all: true,
        },
    )
}

fn row_of<'a>(lines: &'a [String], reference: &str) -> &'a str {
    lines
        .iter()
        .find(|line| line.trim_start().starts_with(&format!("{reference} ")))
        .unwrap_or_else(|| panic!("no row for {reference} in {lines:#?}"))
}

#[test]
fn the_report_opens_with_the_machine() {
    let lines = report(&big_mac(), &judge(&big_mac(), &[]), false);
    assert_eq!(
        lines[0],
        "Apple M5 Pro · 6 performance + 12 efficiency cores · 64 GiB memory"
    );
    assert_eq!(
        lines[1],
        "51.8 GiB for models on the GPU, its Metal working set"
    );
    assert_eq!(lines[2], "412 GiB free on disk");
    assert_eq!(lines[3], "");
    assert_eq!(lines[4], "chat");
}

#[test]
fn each_kind_lists_its_picks_and_what_is_on_the_shelf() {
    let machine = big_mac();
    let lines = report(&machine, &judge(&machine, &["gemma4:31b"]), false);
    let row = row_of(&lines, "qwen3.8:27b");
    assert!(row.contains("17.7 GB"), "{row}");
    assert!(row.contains("fits"), "{row}");
    assert!(row.contains("Qwen's newest"), "{row}");
    assert!(row_of(&lines, "gemma4:31b").contains("on shelf"));
    assert!(!lines.iter().any(|line| line.contains("gpt-oss:120b")));
    assert_eq!(
        lines.last().map(String::as_str),
        Some("hedos pull <name> fetches one · hedos recommend --all shows every model and why")
    );
}

#[test]
fn rows_line_up_across_kinds() {
    let machine = big_mac();
    let lines = report(&machine, &judge(&machine, &[]), false);
    let column = |reference: &str| row_of(&lines, reference).find(" GB").unwrap();
    assert_eq!(column("qwen3.8:27b"), column("qwen3-coder:30b"));
    assert_eq!(column("qwen3.8:27b"), column("stabilityai/sdxl-turbo"));
}

#[test]
fn a_bare_machine_says_once_what_to_install_and_keeps_its_picks() {
    let mut bare = big_mac();
    bare.engines = Engines::default();
    let lines = report(&bare, &judge(&bare, &[]), false);
    assert_eq!(lines[3], OLLAMA_INSTALL_HINT);
    assert_eq!(lines[4], UV_INSTALL_HINT);
    assert_eq!(lines[5], "");
    assert!(row_of(&lines, "qwen3.8:27b").contains("needs Ollama"));
    assert!(row_of(&lines, "mlx-community/Kokoro-82M-bf16").contains("needs uv"));
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.as_str() == OLLAMA_INSTALL_HINT)
            .count(),
        1
    );
}

#[test]
fn a_machine_without_a_gpu_says_where_models_run_and_why_a_kind_is_empty() {
    let machine = small_linux_box();
    let lines = report(&machine, &judge(&machine, &[]), false);
    assert_eq!(lines[0], "8 cores · 8 GiB memory");
    assert_eq!(
        lines[1],
        "no GPU found: models run on the processor, from memory"
    );
    let after = |heading: &str| {
        let index = lines.iter().position(|line| line == heading).unwrap();
        lines[index + 1].clone()
    };
    assert_eq!(
        after("voice"),
        "  nothing of this kind runs on this machine"
    );
    assert_eq!(after("image"), "  nothing of this kind fits this machine");
}

#[test]
fn all_shows_every_model_and_where_it_stands() {
    let machine = big_mac();
    let lines = report(&machine, &judge(&machine, &[]), true);
    assert!(row_of(&lines, "gpt-oss:120b").contains("too big"));
    assert!(row_of(&lines, "qwen3.5:0.8b").contains("also fits"));
    assert_eq!(
        lines.last().map(String::as_str),
        Some("hedos pull <name> fetches one")
    );
    let linux = small_linux_box();
    let lines = report(&linux, &judge(&linux, &[]), true);
    assert!(row_of(&lines, "mlx-community/Kokoro-82M-bf16").contains("can't run here"));
}

#[test]
fn a_card_that_cannot_hold_a_model_says_it_spills() {
    let machine = Machine {
        os: Os::Linux,
        arch: "x86_64".to_owned(),
        devices: vec![Device {
            name: "NVIDIA GeForce RTX 4070".to_owned(),
            kind: DeviceKind::Discrete,
            memory_bytes: 12 * GIB,
        }],
        devices_from: DevicesFrom::NvidiaSmi,
        engines: every_engine(),
        ..Machine::with_memory(64 * GIB)
    };
    let lines = report(&machine, &judge(&machine, &[]), true);
    assert_eq!(
        lines[1],
        "12 GiB for models on NVIDIA GeForce RTX 4070 (nvidia-smi); a larger model runs partly from memory"
    );
    assert!(row_of(&lines, "gpt-oss:20b").contains("also spills"));
}

#[test]
fn a_card_too_small_to_use_is_not_where_models_run() {
    let machine = Machine {
        devices: vec![Device {
            name: "AMD Radeon Graphics".to_owned(),
            kind: DeviceKind::Discrete,
            memory_bytes: 512 << 20,
        }],
        devices_from: DevicesFrom::AmdSysfs,
        ..small_linux_box()
    };
    let lines = report(&machine, &judge(&machine, &[]), false);
    assert_eq!(
        lines[1],
        "no GPU found: models run on the processor, from memory"
    );
}

#[test]
fn free_disk_is_one_figure_unless_the_stores_differ() {
    let mut machine = big_mac();
    machine.free_disk = free_disk(412, 30);
    let lines = report(&machine, &judge(&machine, &[]), false);
    assert_eq!(
        lines[2],
        "412 GiB free for ollama · 30 GiB free for huggingface"
    );
    machine.free_disk = Vec::new();
    let lines = report(&machine, &judge(&machine, &[]), false);
    assert_eq!(lines[2], "");
}

#[test]
fn the_json_carries_the_machine_and_each_shown_model() {
    let mut bare = big_mac();
    bare.engines = Engines::default();
    let judged = judge(&bare, &[]);
    let value = document(&bare, &judged, false);
    let machine = &value["machine"];
    assert_eq!(machine["chip"], "Apple M5 Pro");
    assert_eq!(machine["devices_from"], "metal");
    assert_eq!(machine["models_budget_bytes"], 53084_u64 << 20);
    assert_eq!(machine["devices"][0]["kind"], "unified");
    assert_eq!(machine["engines"]["ollama"], false);
    assert_eq!(machine["free_disk"][0]["provider"], "ollama");
    assert_eq!(machine["free_disk"][0]["bytes"], 412 * GIB);

    let recommendations = value["recommendations"].as_array().unwrap();
    assert!(recommendations.iter().all(|rec| rec["status"] == "pick"));
    let chat = recommendations
        .iter()
        .find(|rec| rec["reference"] == "qwen3.8:27b")
        .unwrap();
    assert_eq!(chat["kind"], "chat");
    assert_eq!(chat["engine"], "ollama");
    assert_eq!(chat["verdict"], "runs_well");
    assert_eq!(chat["placement"], "gpu");
    assert_eq!(chat["download_bytes"], 17_741_871_939_u64);
    assert_eq!(chat["notes"][0]["kind"], "install");
    assert_eq!(chat["notes"][0]["hint"], OLLAMA_INSTALL_HINT);

    let all = document(&bare, &judged, true);
    assert_eq!(
        all["recommendations"].as_array().unwrap().len(),
        judged.len()
    );
}

#[test]
fn kinds_parse_by_name() {
    assert_eq!(parse_kind("code"), Ok(InstallCategory::Code));
    assert_eq!(parse_kind("Image"), Ok(InstallCategory::Image));
    let error = parse_kind("speech").unwrap_err();
    assert_eq!(
        error,
        "speech is not a kind. use one of chat, code, voice, image"
    );
}
