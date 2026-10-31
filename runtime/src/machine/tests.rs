use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kernel::install::InstallProviderId;
use kernel::machine::{Cores, DeviceKind, DevicesFrom, Engine, FreeDisk, Os, Placement};

use super::{Host, available_memory, nvidia_driver_under, probe_with, torch_on_the_cpu_under};

const GIB: u64 = 1 << 30;

/// A machine whose every reading is set by the test. A program it can find
/// prints what `outputs` holds for it, or fails when it holds nothing.
struct FakeHost {
    os: Os,
    arch: &'static str,
    metal: Option<u64>,
    reclaimable: Option<u64>,
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
            reclaimable: None,
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

    fn reclaimable_memory(&self) -> Option<u64> {
        self.reclaimable
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

/// This Mac, as the probe reads it, agrees with what Metal says: its working
/// set where there is a GPU, and something other than Metal on a virtual
/// machine without one.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn this_mac_is_read_as_metal_reports_it() {
    let machine = super::probe();
    assert!(machine.chip.is_some());
    match crate::metal::recommended_working_set() {
        Some(bytes) => {
            assert_eq!(machine.devices_from, DevicesFrom::Metal);
            let device = &machine.devices[0];
            assert_eq!(device.kind, DeviceKind::Unified);
            assert_eq!(device.memory_bytes, bytes);
        }
        None => assert_ne!(machine.devices_from, DevicesFrom::Metal),
    }
}

#[test]
fn available_memory_is_linuxs_own_figure_within_the_tightest_cgroup_limit() {
    let mut host = FakeHost::new(Os::Linux, "x86_64");
    host.files.insert(
        PathBuf::from("/proc/meminfo"),
        "MemTotal: 8388608 kB\nMemAvailable: 6291456 kB\n",
    );
    assert_eq!(available_memory(&host), Some(6 * GIB));

    // A service's own cgroup, limited above it.
    host.files.insert(
        PathBuf::from("/proc/self/cgroup"),
        "0::/system.slice/hedos.service\n",
    );
    for (dir, max, current) in [
        (
            "/sys/fs/cgroup/system.slice/hedos.service",
            "max\n",
            "1073741824\n",
        ),
        (
            "/sys/fs/cgroup/system.slice",
            "4294967296\n",
            "1073741824\n",
        ),
        ("/sys/fs/cgroup", "max\n", "0\n"),
    ] {
        host.files
            .insert(PathBuf::from(dir).join("memory.max"), max);
        host.files
            .insert(PathBuf::from(dir).join("memory.current"), current);
    }
    assert_eq!(available_memory(&host), Some(3 * GIB));

    // cgroup v1's memory controller.
    let mut v1 = FakeHost::new(Os::Linux, "x86_64");
    v1.files
        .insert(PathBuf::from("/proc/meminfo"), "MemAvailable: 6291456 kB\n");
    v1.files
        .insert(PathBuf::from("/proc/self/cgroup"), "9:memory:/docker/abc\n");
    v1.files.insert(
        PathBuf::from("/sys/fs/cgroup/memory/docker/abc/memory.limit_in_bytes"),
        "2147483648\n",
    );
    v1.files.insert(
        PathBuf::from("/sys/fs/cgroup/memory/docker/abc/memory.usage_in_bytes"),
        "0\n",
    );
    assert_eq!(available_memory(&v1), Some(2 * GIB));

    // A memory hierarchy mounted elsewhere, showing a cgroup of its own.
    v1.files.insert(
        PathBuf::from("/proc/self/mountinfo"),
        "40 24 0:35 /docker /sys/fs/cgroup/cpu,memory rw - cgroup cgroup rw,cpu,memory\n",
    );
    v1.files.insert(
        PathBuf::from("/sys/fs/cgroup/cpu,memory/abc/memory.limit_in_bytes"),
        "1073741824\n",
    );
    v1.files.insert(
        PathBuf::from("/sys/fs/cgroup/cpu,memory/abc/memory.usage_in_bytes"),
        "0\n",
    );
    assert_eq!(available_memory(&v1), Some(GIB));
    assert_eq!(available_memory(&FakeHost::new(Os::Macos, "aarch64")), None);
    let mac = FakeHost {
        reclaimable: Some(25 * GIB),
        ..FakeHost::new(Os::Macos, "aarch64")
    };
    assert_eq!(available_memory(&mac), Some(25 * GIB));
    let linux = FakeHost {
        reclaimable: Some(25 * GIB),
        ..FakeHost::new(Os::Linux, "x86_64")
    };
    assert_eq!(
        available_memory(&linux),
        None,
        "macOS's reading, not Linux's"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn this_mac_says_how_much_memory_is_free_or_reclaimable() {
    use super::host::SystemHost;
    let reclaimable = crate::sys::reclaimable_memory().expect("the kernel says");
    let total = SystemHost.memory_bytes();
    assert!(
        reclaimable > 0 && reclaimable <= total,
        "{reclaimable} of {total}"
    );
    // In the units `vm_stat` reads, the kernel's own page size: within a
    // quarter, as pages move between the two reads.
    let said = std::process::Command::new("vm_stat")
        .output()
        .expect("vm_stat");
    let said = String::from_utf8_lossy(&said.stdout);
    let page: u64 = said
        .split("page size of ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|size| size.parse().ok())
        .expect("a page size");
    let pages = |label: &str| -> u64 {
        said.lines()
            .find(|line| line.starts_with(label))
            .and_then(|line| line.rsplit(' ').next())
            .and_then(|count| count.trim_end_matches('.').parse().ok())
            .expect("a count")
    };
    // `vm_stat` prints speculative pages apart from free ones; the kernel
    // counts them as free.
    let expected =
        (pages("Pages free:") + pages("Pages speculative:") + pages("Pages inactive:")) * page;
    let (low, high) = (expected - expected / 4, expected + expected / 4);
    assert!(
        (low..=high).contains(&reclaimable),
        "{reclaimable} against vm_stat's {expected}"
    );
}

#[test]
fn nvidia_s_driver_is_seen_from_its_kernel_module_or_wsl_s_cuda_library() {
    let root = std::env::temp_dir().join(format!("hedos-nvidia-driver-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(!nvidia_driver_under(&root));

    let module = root.join("proc/driver/nvidia");
    std::fs::create_dir_all(&module).unwrap();
    assert!(!nvidia_driver_under(&root));
    std::fs::write(module.join("version"), "NVRM version: 580.65.06").unwrap();
    assert!(nvidia_driver_under(&root));
    std::fs::remove_dir_all(root.join("proc")).unwrap();

    let wsl = root.join("usr/lib/wsl/lib");
    std::fs::create_dir_all(&wsl).unwrap();
    assert!(!nvidia_driver_under(&root));
    std::fs::write(wsl.join("libcuda.so.1"), "").unwrap();
    assert!(nvidia_driver_under(&root));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn torch_runs_on_the_cpu_only_on_linux_without_nvidia_s_driver() {
    let root = std::env::temp_dir().join(format!("hedos-torch-cpu-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    assert!(torch_on_the_cpu_under(Os::Linux, &root));
    assert!(!torch_on_the_cpu_under(Os::Macos, &root));
    let module = root.join("proc/driver/nvidia");
    std::fs::create_dir_all(&module).unwrap();
    std::fs::write(module.join("version"), "NVRM version: 580.65.06").unwrap();
    assert!(!torch_on_the_cpu_under(Os::Linux, &root));
    let _ = std::fs::remove_dir_all(&root);
}
