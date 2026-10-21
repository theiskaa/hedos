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

use std::path::Path;

use kernel::machine::{
    CgroupMount, CgroupPaths, Device, DeviceKind, DevicesFrom, Engines, Machine, Os, amd_device,
    cgroup_ancestors, cgroup_room, is_apple_silicon, launch_memory, parse_cgroup_mounts,
    parse_list_devices, parse_mem_available, parse_nvidia_smi, parse_proc_cgroup,
};

pub(crate) use host::{Host, Stream, SystemHost, run_bounded_reading};

const NVIDIA_SMI_ARGS: [&str; 2] = [
    "--query-gpu=name,memory.total",
    "--format=csv,noheader,nounits",
];

/// This machine. The hardware is read once per process, so the screen, the
/// picker and `hedos ls` judge against the same figures; the engines and the
/// free disk are read again on every call, since either can change while the
/// shelf stays open.
pub fn probe() -> Machine {
    let host = SystemHost;
    Machine {
        engines: engines(&host),
        free_disk: host.free_disk(),
        ..hardware().clone()
    }
}

/// This machine's hardware, read once per process.
fn hardware() -> &'static Machine {
    static HARDWARE: OnceLock<Machine> = OnceLock::new();
    HARDWARE.get_or_init(|| probe_with(&SystemHost))
}

/// Read this machine's hardware now, off the calling thread, so the first
/// request a server takes does not wait on it (finding cards can run
/// `nvidia-smi` or `llama-server --list-devices`).
pub fn warm_hardware() {
    std::thread::spawn(|| {
        hardware();
    });
}

/// The memory the llama-server hedos would run can take as it starts, read
/// now: see [`launch_memory`].
pub fn llama_launch_memory() -> Option<u64> {
    launch_memory(
        hardware(),
        available_memory(&SystemHost),
        llama_accelerated(),
    )
}

/// Whether the llama-server hedos would run uses a GPU: not a CPU build hedos
/// carries or installed. One named in the settings or found on the PATH is
/// taken to use what the machine has.
fn llama_accelerated() -> bool {
    use crate::engines::llama_cpp::{Source, installed, located};
    use kernel::engines::llama_cpp::{InstallSource, Variant};
    let Some(server) = located() else {
        return true;
    };
    let recorded = |source| {
        installed(&crate::boot::HedosDirs::detect().data, source)
            .filter(|(dir, _)| server.path.parent() == Some(dir.as_path()))
            .map(|(_, record)| record.variant)
    };
    let variant = match server.from {
        // A build an earlier hedos carried and unpacked says what it is in
        // its record.
        Source::Bundled => crate::engines::bundled::bundled()
            .map(|carried| carried.variant)
            .or_else(|| recorded(InstallSource::Bundled)),
        Source::Hedos => recorded(InstallSource::Requested),
        Source::Config | Source::Path => None,
    };
    variant != Some(Variant::Cpu)
}

/// The least room under any memory limit on this process's cgroup or those
/// above it: cgroup v2's `memory.max`, else v1's `memory.limit_in_bytes`.
fn cgroup_limit(host: &dyn Host, paths: &CgroupPaths) -> Option<u64> {
    // Where the hierarchies are mounted, else where systemd and Docker put
    // them.
    let (unified_mount, memory_mount) = host
        .read(Path::new("/proc/self/mountinfo"))
        .map(|text| parse_cgroup_mounts(&text))
        .unwrap_or_default();
    let mounted = |at: &str| CgroupMount {
        root: "/".to_owned(),
        mount_point: at.to_owned(),
    };
    let unified_mount = unified_mount.unwrap_or_else(|| mounted("/sys/fs/cgroup"));
    let memory_mount = memory_mount.unwrap_or_else(|| mounted("/sys/fs/cgroup/memory"));
    let tightest = |mount: &CgroupMount, path: &str, limit: &str, usage: &str| {
        cgroup_ancestors(path)
            .iter()
            .filter_map(|cgroup| {
                let dir = mount.dir_of(cgroup)?;
                let dir = Path::new(&dir);
                cgroup_room(&host.read(&dir.join(limit))?, &host.read(&dir.join(usage))?)
            })
            .min()
    };
    let unified = paths
        .unified
        .as_deref()
        .and_then(|path| tightest(&unified_mount, path, "memory.max", "memory.current"));
    let v1 = paths.memory.as_deref().and_then(|path| {
        tightest(
            &memory_mount,
            path,
            "memory.limit_in_bytes",
            "memory.usage_in_bytes",
        )
    });
    unified.into_iter().chain(v1).min()
}

/// The memory a new process can take right now without swapping: Linux's
/// `MemAvailable`, within the room left under this process's cgroup limit;
/// macOS's free and inactive pages. `None` elsewhere.
pub(crate) fn available_memory(host: &dyn Host) -> Option<u64> {
    if host.os() == Os::Macos {
        return host.reclaimable_memory();
    }
    if host.os() != Os::Linux {
        return None;
    }
    let available = host
        .read(Path::new("/proc/meminfo"))
        .as_deref()
        .and_then(parse_mem_available);
    let room = host
        .read(Path::new("/proc/self/cgroup"))
        .map(|text| parse_proc_cgroup(&text))
        .and_then(|paths| cgroup_limit(host, &paths));
    match (available, room) {
        (Some(available), Some(room)) => Some(available.min(room)),
        (available, room) => available.or(room),
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
        llama_cpp: host.llama_server().is_some(),
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
        .llama_server()
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
