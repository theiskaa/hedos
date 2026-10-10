//! Reading this machine: its memory, the accelerator a model would run on and
//! how much of it a model may take, which engines are installed, and the free
//! disk where pulls land.
//!
//! No figure depends on an engine being installed. The device comes from Metal
//! on a Mac, `nvidia-smi` or the AMD driver on Linux, and only past those from
//! `llama-server --list-devices`. Every source is optional: one that is missing,
//! fails, or times out has nothing to say, and a machine where all of them are
//! silent is still judged, against its memory.

mod host;

use std::sync::OnceLock;

use kernel::machine::{
    Device, DeviceKind, DevicesFrom, Engines, Machine, Os, amd_device, is_apple_silicon,
    parse_list_devices, parse_nvidia_smi,
};

pub(crate) use host::{Host, SystemHost};

const NVIDIA_SMI_ARGS: [&str; 2] = [
    "--query-gpu=name,memory.total",
    "--format=csv,noheader,nounits",
];

/// This machine. The hardware is read once per process, so the screen, the
/// picker and `hedos ls` judge against the same figures; the engines and the
/// free disk are read again on every call, since either can change while the
/// shelf stays open.
pub fn probe() -> Machine {
    static HARDWARE: OnceLock<Machine> = OnceLock::new();
    let host = SystemHost;
    let hardware = HARDWARE.get_or_init(|| probe_with(&host));
    Machine {
        engines: engines(&host),
        free_disk: host.free_disk(),
        ..hardware.clone()
    }
}

/// The machine `host` describes.
pub(crate) fn probe_with(host: &dyn Host) -> Machine {
    let os = host.os();
    let arch = host.arch();
    let chip = host.chip();
    let (devices, devices_from) = devices(host, os, &arch, chip.as_deref());
    Machine {
        os,
        arch,
        chip,
        cores: host.cores(),
        memory_bytes: host.memory_bytes(),
        devices,
        devices_from,
        engines: engines(host),
        free_disk: host.free_disk(),
    }
}

/// Which engines `host` has installed.
fn engines(host: &dyn Host) -> Engines {
    Engines {
        ollama: host.ollama(),
        llama_cpp: host.find("llama-server").is_some(),
        uv: host.uv(),
    }
}

/// The accelerators, from the first source that reports any a model can be
/// placed on. A card too small for that (an APU's carve-out) is dropped, so a
/// later source still gets its turn.
fn devices(host: &dyn Host, os: Os, arch: &str, chip: Option<&str>) -> (Vec<Device>, DevicesFrom) {
    let apple_silicon = is_apple_silicon(os, arch);
    if apple_silicon && let Some(bytes) = host.metal_working_set() {
        let device = Device {
            name: chip.unwrap_or("Apple GPU").to_owned(),
            kind: DeviceKind::Unified,
            memory_bytes: bytes,
        };
        return (vec![device], DevicesFrom::Metal);
    }
    // An Intel Mac's GPU is one no engine here runs a model on, though
    // llama.cpp would list it as a Metal device.
    if os == Os::Macos && !apple_silicon {
        return (Vec::new(), DevicesFrom::None);
    }
    let usable = |devices: Vec<Device>| -> Vec<Device> {
        devices.into_iter().filter(Device::usable).collect()
    };
    if os == Os::Linux {
        let nvidia = host
            .find("nvidia-smi")
            .and_then(|smi| host.run(&smi, &NVIDIA_SMI_ARGS))
            .map(|text| usable(parse_nvidia_smi(&text)))
            .unwrap_or_default();
        if !nvidia.is_empty() {
            return (nvidia, DevicesFrom::NvidiaSmi);
        }
        let amd = usable(amd_cards(host));
        if !amd.is_empty() {
            return (amd, DevicesFrom::AmdSysfs);
        }
    }
    let listed = host
        .find("llama-server")
        .and_then(|server| host.run(&server, &["--list-devices"]))
        .map(|text| usable(parse_list_devices(&text)))
        .unwrap_or_default();
    if !listed.is_empty() {
        return (listed, DevicesFrom::LlamaCpp);
    }
    (Vec::new(), DevicesFrom::None)
}

/// The AMD cards the driver describes under `/sys/class/drm`.
fn amd_cards(host: &dyn Host) -> Vec<Device> {
    host.drm_cards()
        .into_iter()
        .filter_map(|card| {
            let device = card.join("device");
            let vram = host.read(&device.join("mem_info_vram_total"))?;
            let name = host.read(&device.join("product_name"));
            amd_device(name.as_deref(), &vram)
        })
        .collect()
}

#[cfg(test)]
mod tests;
