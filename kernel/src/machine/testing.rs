//! Machines the kernel's tests judge against.

use super::{Device, DeviceKind, DevicesFrom, Engines, Machine, Os};

pub(crate) const GIB: u64 = 1 << 30;

/// Every engine installed.
pub(crate) fn every_engine() -> Engines {
    Engines {
        ollama: true,
        llama_cpp: true,
        uv: true,
    }
}

/// An Apple Silicon Mac with `memory_gib` of memory, a Metal working set of
/// `working_set_mib`, and every engine.
pub(crate) fn apple_silicon(memory_gib: u64, working_set_mib: u64) -> Machine {
    Machine {
        os: Os::Macos,
        arch: "aarch64".to_owned(),
        chip: Some("Apple M5 Pro".to_owned()),
        devices: vec![Device {
            name: "Apple M5 Pro".to_owned(),
            kind: DeviceKind::Unified,
            memory_bytes: working_set_mib << 20,
        }],
        devices_from: DevicesFrom::Metal,
        engines: every_engine(),
        ..Machine::with_memory(memory_gib * GIB)
    }
}

/// A 64 GiB M5 Pro, whose Metal working set is 53084 MiB.
pub(crate) fn m5_pro() -> Machine {
    apple_silicon(64, 53084)
}

/// A discrete card named `name` with `gib` of memory.
pub(crate) fn card(name: &str, gib: u64) -> Device {
    Device {
        name: name.to_owned(),
        kind: DeviceKind::Discrete,
        memory_bytes: gib * GIB,
    }
}

/// A Linux box with `memory_gib` of memory, `cards`, and every engine.
pub(crate) fn linux(memory_gib: u64, cards: Vec<Device>) -> Machine {
    Machine {
        os: Os::Linux,
        arch: "x86_64".to_owned(),
        devices: cards,
        devices_from: DevicesFrom::NvidiaSmi,
        engines: every_engine(),
        ..Machine::with_memory(memory_gib * GIB)
    }
}
