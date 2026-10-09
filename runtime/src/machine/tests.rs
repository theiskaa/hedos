use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kernel::install::InstallProviderId;
use kernel::machine::{Cores, DeviceKind, DevicesFrom, Engine, FreeDisk, Os, Placement};

use super::{Host, probe_with};

const GIB: u64 = 1 << 30;

/// A machine whose every reading is set by the test. A program it can find
/// prints what `outputs` holds for it, or fails when it holds nothing.
struct FakeHost {
    os: Os,
    arch: &'static str,
    metal: Option<u64>,
    programs: Vec<&'static str>,
    outputs: HashMap<&'static str, &'static str>,
    files: HashMap<PathBuf, &'static str>,
    cards: Vec<PathBuf>,
    ollama: bool,
    uv: bool,
    ran: Mutex<Vec<String>>,
}

impl FakeHost {
    fn new(os: Os, arch: &'static str) -> Self {
        Self {
            os,
            arch,
            metal: None,
            programs: Vec::new(),
            outputs: HashMap::new(),
            files: HashMap::new(),
            cards: Vec::new(),
            ollama: false,
            uv: false,
            ran: Mutex::new(Vec::new()),
        }
    }

    fn with(mut self, program: &'static str, output: Option<&'static str>) -> Self {
        self.programs.push(program);
        if let Some(output) = output {
            self.outputs.insert(program, output);
        }
        self
    }

    fn ran(&self) -> Vec<String> {
        self.ran.lock().unwrap().clone()
    }
}

impl Host for FakeHost {
    fn os(&self) -> Os {
        self.os
    }
    fn arch(&self) -> String {
        self.arch.to_owned()
    }
    fn chip(&self) -> Option<String> {
        Some("Test Chip".to_owned())
    }
    fn cores(&self) -> Cores {
        Cores {
            performance: Some(6),
            efficiency: Some(12),
            logical: 18,
        }
    }
    fn memory_bytes(&self) -> u64 {
        64 * GIB
    }
    fn free_disk(&self) -> Vec<FreeDisk> {
        vec![FreeDisk {
            provider: InstallProviderId::ollama(),
            bytes: 400 * GIB,
        }]
    }
    fn metal_working_set(&self) -> Option<u64> {
        self.metal
    }
    fn find(&self, program: &str) -> Option<PathBuf> {
        self.programs
            .contains(&program)
            .then(|| PathBuf::from("/bin").join(program))
    }
    fn run(&self, program: &Path, _args: &[&str]) -> Option<String> {
        let name = program.file_name()?.to_str()?;
        self.ran.lock().unwrap().push(name.to_owned());
        self.outputs.get(name).map(|output| (*output).to_owned())
    }
    fn read(&self, path: &Path) -> Option<String> {
        self.files.get(path).map(|text| (*text).to_owned())
    }
    fn drm_cards(&self) -> Vec<PathBuf> {
        self.cards.clone()
    }
    fn ollama(&self) -> bool {
        self.ollama
    }
    fn uv(&self) -> bool {
        self.uv
    }
}

const MAC_LIST: &str = "Available devices:\n  BLAS: Accelerate (0 MiB, 0 MiB free)\n  MTL0: Apple M5 Pro (53084 MiB, 53083 MiB free)\n";
const CUDA_LIST: &str =
    "Available devices:\n  CUDA0: NVIDIA GeForce RTX 4090 (24210 MiB, 23700 MiB free)\n";

#[test]
fn apple_silicon_reads_metal_and_never_asks_llama_cpp() {
    let mut host = FakeHost::new(Os::Macos, "aarch64").with("llama-server", Some(MAC_LIST));
    host.metal = Some(53084 << 20);
    let machine = probe_with(&host);
    assert_eq!(machine.devices_from, DevicesFrom::Metal);
    assert_eq!(machine.devices.len(), 1);
    assert_eq!(machine.devices[0].name, "Test Chip");
    assert_eq!(machine.devices[0].kind, DeviceKind::Unified);
    assert_eq!(machine.devices[0].memory_bytes, 53084 << 20);
    assert!(machine.engines.llama_cpp);
    assert!(host.ran().is_empty());
}

#[test]
fn apple_silicon_without_metal_falls_back_to_llama_cpp() {
    let host = FakeHost::new(Os::Macos, "aarch64").with("llama-server", Some(MAC_LIST));
    let machine = probe_with(&host);
    assert_eq!(machine.devices_from, DevicesFrom::LlamaCpp);
    assert_eq!(machine.devices[0].memory_bytes, 53084 << 20);
}

#[test]
fn an_intel_mac_has_no_device_even_when_llama_cpp_lists_one() {
    let host = FakeHost::new(Os::Macos, "x86_64").with("llama-server", Some(MAC_LIST));
    let machine = probe_with(&host);
    assert_eq!(machine.devices_from, DevicesFrom::None);
    assert!(machine.devices.is_empty());
    assert!(host.ran().is_empty());
    assert_eq!(
        machine.budget(Engine::Ollama).unwrap().placement,
        Placement::Cpu
    );
}

#[test]
fn linux_reads_nvidia_smi_before_llama_cpp() {
    let host = FakeHost::new(Os::Linux, "x86_64")
        .with("nvidia-smi", Some("NVIDIA GeForce RTX 4090, 24564\n"))
        .with("llama-server", Some(CUDA_LIST));
    let machine = probe_with(&host);
    assert_eq!(machine.devices_from, DevicesFrom::NvidiaSmi);
    assert_eq!(machine.devices[0].memory_bytes, 24564 << 20);
    assert_eq!(host.ran(), vec!["nvidia-smi"]);
}

#[test]
fn a_failing_nvidia_smi_falls_through_to_amd_then_llama_cpp() {
    let mut host = FakeHost::new(Os::Linux, "x86_64")
        .with("nvidia-smi", None)
        .with("llama-server", Some(CUDA_LIST));
    let machine = probe_with(&host);
    assert_eq!(machine.devices_from, DevicesFrom::LlamaCpp);
    assert_eq!(host.ran(), vec!["nvidia-smi", "llama-server"]);

    host.cards = vec![PathBuf::from("/sys/class/drm/card0")];
    host.files.insert(
        PathBuf::from("/sys/class/drm/card0/device/mem_info_vram_total"),
        "25753026560\n",
    );
    host.files.insert(
        PathBuf::from("/sys/class/drm/card0/device/product_name"),
        "Radeon RX 7900 XTX\n",
    );
    let machine = probe_with(&host);
    assert_eq!(machine.devices_from, DevicesFrom::AmdSysfs);
    assert_eq!(machine.devices[0].name, "Radeon RX 7900 XTX");
}

#[test]
fn a_card_too_small_to_use_leaves_the_turn_to_the_next_source() {
    let host = FakeHost::new(Os::Linux, "x86_64")
        .with("nvidia-smi", Some("NVIDIA GeForce MX150, 512\n"))
        .with("llama-server", Some(CUDA_LIST));
    let machine = probe_with(&host);
    assert_eq!(machine.devices_from, DevicesFrom::LlamaCpp);
    assert_eq!(machine.devices[0].name, "NVIDIA GeForce RTX 4090");
}

#[test]
fn a_machine_where_every_source_is_silent_is_judged_on_memory() {
    for (os, arch) in [
        (Os::Macos, "aarch64"),
        (Os::Macos, "x86_64"),
        (Os::Linux, "x86_64"),
        (Os::Linux, "aarch64"),
    ] {
        let host = FakeHost::new(os, arch)
            .with("nvidia-smi", None)
            .with("llama-server", None);
        let machine = probe_with(&host);
        assert_eq!(machine.devices_from, DevicesFrom::None, "{os:?} {arch}");
        assert!(machine.devices.is_empty());
        assert_eq!(machine.memory_bytes, 64 * GIB);
        let budget = machine.budget(Engine::Ollama).unwrap();
        assert_eq!(budget.bytes, 64 * GIB);
        assert!(machine.fit(Engine::Ollama, Some(GIB as i64)).is_some());
    }
}

#[test]
fn engines_are_read_from_the_host() {
    let bare = probe_with(&FakeHost::new(Os::Linux, "x86_64"));
    assert!(!bare.engines.ollama && !bare.engines.llama_cpp && !bare.engines.uv);

    let mut host = FakeHost::new(Os::Linux, "x86_64").with("llama-server", None);
    host.ollama = true;
    host.uv = true;
    let machine = probe_with(&host);
    assert!(machine.engines.ollama && machine.engines.llama_cpp && machine.engines.uv);
}

#[test]
fn the_rest_of_the_facts_pass_through() {
    let machine = probe_with(&FakeHost::new(Os::Linux, "x86_64"));
    assert_eq!(machine.os, Os::Linux);
    assert_eq!(machine.arch, "x86_64");
    assert_eq!(machine.chip.as_deref(), Some("Test Chip"));
    assert_eq!(machine.cores.performance, Some(6));
    assert_eq!(
        machine.free_disk_for(&InstallProviderId::ollama()),
        Some(400 * GIB)
    );
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn this_mac_reads_its_working_set_from_metal() {
    let machine = super::probe();
    assert_eq!(machine.devices_from, DevicesFrom::Metal);
    let device = &machine.devices[0];
    assert_eq!(device.kind, DeviceKind::Unified);
    assert!(device.memory_bytes > 0 && device.memory_bytes <= machine.memory_bytes);
    assert!(machine.chip.is_some());
}
