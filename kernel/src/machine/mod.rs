//! What this machine can give a model: its memory, the accelerator a model
//! would run on and how much of it a model may take, and which of the engines
//! that serve models are here to do it.
//!
//! Nothing here reads the machine. The runtime probes it and hands the facts
//! over; this decides what they mean for a model of a given size.

mod devices;
#[cfg(test)]
pub(crate) mod testing;

pub use devices::{amd_device, parse_list_devices, parse_nvidia_smi};

use crate::install::provider::InstallProviderId;
use crate::profiles::{FitAssessment, FitVerdict, Scale};
use crate::records::{ModelRecord, RuntimeId};

/// A device smaller than this is not worth placing a model on: an APU's
/// carve-out, or a card with too little memory to hold even the smallest pick.
const MIN_DEVICE_BYTES: u64 = 1 << 30;

/// What `hedos pull` tells someone who has no Ollama, the engine behind every
/// Ollama tag: both the pull and the serving go through its daemon.
pub const OLLAMA_INSTALL_HINT: &str = "Ollama isn't installed. Get it from ollama.com.";
/// What a model llama.cpp serves needs.
pub const LLAMA_CPP_INSTALL_HINT: &str =
    "llama.cpp isn't installed. Install it so `llama-server` is on PATH.";
/// What a model a Python sidecar serves needs.
pub const UV_INSTALL_HINT: &str =
    "uv is required to prepare Python runtimes. Install it from astral.sh/uv.";

/// The operating system, as far as the engines care.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    /// macOS.
    Macos,
    /// Linux.
    Linux,
    /// Anything else.
    Other,
}

impl Os {
    /// The OS this binary was built for.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else if cfg!(target_os = "linux") {
            Os::Linux
        } else {
            Os::Other
        }
    }

    /// The stable string form: `macos`, `linux`, or `other`.
    pub fn as_str(self) -> &'static str {
        match self {
            Os::Macos => "macos",
            Os::Linux => "linux",
            Os::Other => "other",
        }
    }
}

/// Whether `os` on `arch` is a Mac with Apple's own chip, where MLX runs and
/// the GPU shares the system's memory.
pub fn is_apple_silicon(os: Os, arch: &str) -> bool {
    os == Os::Macos && arch == "aarch64"
}

/// The processor's cores. The split is known only where the OS reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cores {
    /// Performance cores.
    pub performance: Option<u32>,
    /// Efficiency cores.
    pub efficiency: Option<u32>,
    /// Every core the OS schedules on.
    pub logical: u32,
}

/// How a device's memory relates to the system's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    /// Shares the system's memory (Apple Silicon); its figure is the share a
    /// model may take of it.
    Unified,
    /// A card with memory of its own.
    Discrete,
}

impl DeviceKind {
    /// The stable string form: `unified` or `discrete`.
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceKind::Unified => "unified",
            DeviceKind::Discrete => "discrete",
        }
    }
}

/// One accelerator a model can run on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// The name it reports (`Apple M5 Pro`, `NVIDIA GeForce RTX 4090`).
    pub name: String,
    /// Unified or discrete.
    pub kind: DeviceKind,
    /// The memory a model may use on it.
    pub memory_bytes: u64,
}

impl Device {
    /// Whether it holds enough to place a model on.
    pub fn usable(&self) -> bool {
        self.memory_bytes >= MIN_DEVICE_BYTES
    }
}

/// Where the device figures came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DevicesFrom {
    /// Metal's recommended working set.
    Metal,
    /// `nvidia-smi`.
    NvidiaSmi,
    /// The AMD driver's sysfs files.
    AmdSysfs,
    /// `llama-server --list-devices`.
    LlamaCpp,
    /// Nothing reported a device.
    None,
}

impl DevicesFrom {
    /// The stable string form.
    pub fn as_str(self) -> &'static str {
        match self {
            DevicesFrom::Metal => "metal",
            DevicesFrom::NvidiaSmi => "nvidia-smi",
            DevicesFrom::AmdSysfs => "amd-sysfs",
            DevicesFrom::LlamaCpp => "llama-cpp",
            DevicesFrom::None => "none",
        }
    }
}

/// Which of the engines that serve models are on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Engines {
    /// Ollama: its binary is installed, or its daemon answers.
    pub ollama: bool,
    /// `llama-server` is on PATH.
    pub llama_cpp: bool,
    /// `uv`, which provisions the Python sidecars.
    pub uv: bool,
}

/// The free space where one provider's pulls land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeDisk {
    /// The provider.
    pub provider: InstallProviderId,
    /// The bytes free on the disk its pulls land on.
    pub bytes: u64,
}

/// What serves a model, as far as memory and readiness go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Engine {
    /// The Ollama daemon: Ollama tags, pulled and served through it.
    Ollama,
    /// `llama-server` (and whisper.cpp, which budgets the same way).
    LlamaCpp,
    /// The MLX sidecars (mlx-lm, mlx-vlm, mlx-audio, mflux): Apple Silicon only.
    Mlx,
    /// The PyTorch sidecars (diffusers, embeddings).
    Torch,
    /// Anything else (a remote endpoint, a daemon of its own, a manifest
    /// runtime, or nothing resolved yet): judged against system memory.
    Other,
}

impl Engine {
    /// The stable string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Ollama => "ollama",
            Engine::LlamaCpp => "llama-cpp",
            Engine::Mlx => "mlx",
            Engine::Torch => "torch",
            Engine::Other => "other",
        }
    }

    /// What has to be installed for it to serve, or `None` for
    /// [`Engine::Other`], which needs nothing hedos can name.
    pub fn label(self) -> Option<&'static str> {
        match self {
            Engine::Ollama => Some("Ollama"),
            Engine::LlamaCpp => Some("llama.cpp"),
            Engine::Mlx | Engine::Torch => Some("uv"),
            Engine::Other => None,
        }
    }

    /// The engine a resolved runtime is.
    pub fn of_runtime(runtime: &RuntimeId) -> Self {
        let mlx = [
            RuntimeId::mlx_swift(),
            RuntimeId::mlx_lm(),
            RuntimeId::mlx_vlm(),
            RuntimeId::mlx_audio(),
            RuntimeId::mflux(),
        ];
        if *runtime == RuntimeId::ollama() {
            Engine::Ollama
        } else if *runtime == RuntimeId::llama_cpp() || *runtime == RuntimeId::whisper_cpp() {
            Engine::LlamaCpp
        } else if mlx.contains(runtime) {
            Engine::Mlx
        } else if *runtime == RuntimeId::diffusers() || *runtime == RuntimeId::embeddings() {
            Engine::Torch
        } else {
            Engine::Other
        }
    }

    /// The engine `record` resolved to, or [`Engine::Other`] when it has not.
    pub fn of_record(record: &ModelRecord) -> Self {
        record
            .runtime
            .id
            .as_ref()
            .map_or(Engine::Other, Self::of_runtime)
    }
}

/// Where a model's weights would sit while it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Placement {
    /// All of it on the accelerator.
    Gpu,
    /// What the cards cannot hold runs from system memory: it runs, slower.
    Spill,
    /// On the processor, from system memory.
    Cpu,
}

impl Placement {
    /// The stable string form.
    pub fn as_str(self) -> &'static str {
        match self {
            Placement::Gpu => "gpu",
            Placement::Spill => "spill",
            Placement::Cpu => "cpu",
        }
    }
}

/// The memory an engine may give a model here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// The bytes a model may take.
    pub bytes: u64,
    /// What those bytes measure.
    pub scale: Scale,
    /// Where a model within them runs.
    pub placement: Placement,
    /// Whether the engine runs what the cards cannot hold from system
    /// memory, as llama.cpp and Ollama do with layers.
    pub spills: bool,
}

/// How a model fits, and where it would run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    /// The verdict and the memory it was judged to need.
    pub assessment: FitAssessment,
    /// Where it would run.
    pub placement: Placement,
    /// The memory it was judged against: the card's or the GPU's share, or
    /// all of memory when it runs from there.
    pub budget_bytes: u64,
}

/// Whether an engine is here to serve a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// It is.
    Ready,
    /// It has to be installed first; the hint says how.
    Install(&'static str),
}

/// The facts about this machine that decide what a model can do on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Machine {
    /// The operating system.
    pub os: Os,
    /// The CPU architecture (`aarch64`, `x86_64`).
    pub arch: String,
    /// The processor's name, where it can be read.
    pub chip: Option<String>,
    /// Its cores.
    pub cores: Cores,
    /// Total system memory.
    pub memory_bytes: u64,
    /// The accelerators a model can run on.
    pub devices: Vec<Device>,
    /// Where `devices` came from.
    pub devices_from: DevicesFrom,
    /// Which engines are installed.
    pub engines: Engines,
    /// Free space where each provider's pulls land, where it could be read.
    pub free_disk: Vec<FreeDisk>,
}

impl Default for Machine {
    /// A machine nothing is known about yet: no memory, so nothing fits.
    fn default() -> Self {
        Self::with_memory(0)
    }
}

impl Machine {
    /// A machine known only by its memory: no accelerator, no engines, no
    /// disk figure. Every engine but MLX is judged against `memory_bytes` on
    /// the system scale.
    pub fn with_memory(memory_bytes: u64) -> Self {
        Self {
            os: Os::Other,
            arch: String::new(),
            chip: None,
            cores: Cores::default(),
            memory_bytes,
            devices: Vec::new(),
            devices_from: DevicesFrom::None,
            engines: Engines::default(),
            free_disk: Vec::new(),
        }
    }

    /// Whether this is a Mac with Apple's own chip, where MLX runs.
    pub fn apple_silicon(&self) -> bool {
        is_apple_silicon(self.os, &self.arch)
    }

    /// The free space where `provider`'s pulls land, where it could be read.
    pub fn free_disk_for(&self, provider: &InstallProviderId) -> Option<u64> {
        self.free_disk
            .iter()
            .find(|free| free.provider == *provider)
            .map(|free| free.bytes)
    }

    /// The unified device, if there is one.
    fn unified(&self) -> Option<&Device> {
        self.devices
            .iter()
            .find(|device| device.kind == DeviceKind::Unified && device.memory_bytes > 0)
    }

    /// The discrete cards big enough to place a model on.
    fn cards(&self) -> impl Iterator<Item = &Device> {
        self.devices
            .iter()
            .filter(|device| device.kind == DeviceKind::Discrete && device.usable())
    }

    /// System memory on the system scale, on the processor unless the memory
    /// is the accelerator's too.
    fn system_budget(&self) -> Budget {
        Budget {
            bytes: self.memory_bytes,
            scale: Scale::System,
            placement: if self.apple_silicon() {
                Placement::Gpu
            } else {
                Placement::Cpu
            },
            spills: false,
        }
    }

    /// The memory `engine` may give a model on this machine, or `None` when it
    /// cannot run here at all.
    pub fn budget(&self, engine: Engine) -> Option<Budget> {
        if engine == Engine::Mlx && !self.apple_silicon() {
            return None;
        }
        if engine == Engine::Other {
            return Some(Budget {
                placement: Placement::Cpu,
                ..self.system_budget()
            });
        }
        if let Some(device) = self.unified() {
            // PyTorch's Metal backend allocates from all of memory, not from
            // the working set the Metal engines keep to.
            if engine == Engine::Torch {
                return Some(self.system_budget());
            }
            return Some(Budget {
                bytes: device.memory_bytes,
                scale: Scale::Accelerator,
                placement: Placement::Gpu,
                spills: false,
            });
        }
        let cards: Vec<u64> = self.cards().map(|card| card.memory_bytes).collect();
        if cards.is_empty() {
            return Some(self.system_budget());
        }
        Some(match engine {
            // A PyTorch pipeline runs on one card, and the sidecars here do
            // not offload it in pieces.
            Engine::Torch => Budget {
                bytes: cards.iter().copied().max().unwrap_or_default(),
                scale: Scale::Accelerator,
                placement: Placement::Gpu,
                spills: false,
            },
            // llama.cpp and Ollama split layers across every card.
            _ => Budget {
                bytes: cards.iter().sum(),
                scale: Scale::Accelerator,
                placement: Placement::Gpu,
                spills: true,
            },
        })
    }

    /// How a model of `footprint_bytes` fits when `engine` serves it, or
    /// `None` when its size is unknown or the engine cannot run here.
    ///
    /// An engine that spills is never judged worse than the processor alone
    /// would be: a model the cards cannot hold well is judged against all of
    /// memory instead, where most of it would run, whenever that reads
    /// better or the cards cannot hold it at all. A small card then still
    /// lets a large model fit.
    pub fn fit(&self, engine: Engine, footprint_bytes: Option<i64>) -> Option<Fit> {
        let budget = self.budget(engine)?;
        let on_cards = Fit {
            assessment: FitVerdict::assess_in(footprint_bytes, budget.bytes, budget.scale)?,
            placement: budget.placement,
            budget_bytes: budget.bytes,
        };
        if !budget.spills || on_cards.assessment.verdict == FitVerdict::RunsWell {
            return Some(on_cards);
        }
        // Past the cards altogether, all of memory is the ceiling a model is
        // too big for, whatever it reads there.
        let too_big_for_cards = on_cards.assessment.verdict == FitVerdict::TooLarge;
        let in_memory = FitVerdict::assess_in(footprint_bytes, self.memory_bytes, Scale::System)
            .filter(|assessment| {
                too_big_for_cards || assessment.verdict < on_cards.assessment.verdict
            });
        Some(in_memory.map_or(on_cards, |assessment| Fit {
            assessment,
            placement: Placement::Spill,
            budget_bytes: self.memory_bytes,
        }))
    }

    /// How a model of `footprint_bytes` fits under `engine`, or under
    /// [`Engine::Other`] (all of memory) when `engine` cannot run here.
    /// `None` only when the size is unknown or nothing is known of memory.
    pub fn fit_on(&self, engine: Engine, footprint_bytes: Option<i64>) -> Option<Fit> {
        self.fit(engine, footprint_bytes)
            .or_else(|| self.fit(Engine::Other, footprint_bytes))
    }

    /// How `record` fits, judged by what serving it loads on the engine it
    /// resolved to.
    pub fn fit_record(&self, record: &ModelRecord) -> Option<Fit> {
        self.fit_on(Engine::of_record(record), record.serving_size())
    }

    /// The memory a model may use here, as one figure for a headline: what
    /// Ollama and llama.cpp may give it, the cards' or the GPU's share where
    /// there is one, else all of memory.
    pub fn models_budget_bytes(&self) -> u64 {
        self.budget(Engine::Ollama)
            .map_or(self.memory_bytes, |budget| budget.bytes)
    }

    /// Whether `engine` is installed to serve a model.
    pub fn readiness(&self, engine: Engine) -> Readiness {
        let (ready, hint) = match engine {
            Engine::Ollama => (self.engines.ollama, OLLAMA_INSTALL_HINT),
            Engine::LlamaCpp => (self.engines.llama_cpp, LLAMA_CPP_INSTALL_HINT),
            Engine::Mlx | Engine::Torch => (self.engines.uv, UV_INSTALL_HINT),
            Engine::Other => return Readiness::Ready,
        };
        if ready {
            Readiness::Ready
        } else {
            Readiness::Install(hint)
        }
    }
}

#[cfg(test)]
mod tests;
