//! Installing llama.cpp from the CLI, the same way from `hedos engines
//! install` and from the question `hedos pull` asks when a model it pulled
//! needs it: say what will be fetched, from where, how big and where it goes,
//! and fetch it only on a yes.

use std::io::IsTerminal;
use std::sync::Mutex;

use kernel::engines::llama_cpp::{Asset, PINNED, Variant};
use kernel::machine::{LLAMA_CPP_INSTALL_HINT, Machine, Os};
use kernel::records::{ModelRecord, ModelState, RuntimeId, SourceKind, format_bytes};
use runtime::boot::HedosDirs;
use runtime::engines::bundled::bundled;
use runtime::engines::install::{InstallProgress, install};
use runtime::engines::llama_cpp::{LlamaServer, Source, hedos_root, located};
use runtime::engines::published_builds_run_here;

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::spinner::Spinner;

/// An engine a model on the shelf needs and this machine lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineNeed {
    /// It resolved to llama.cpp, and no `llama-server` is here: hedos can
    /// install one when asked.
    LlamaCpp,
    /// It is in Ollama's own format, which only Ollama runs, and Ollama is
    /// not here: nothing serves it.
    Ollama,
}

/// The engine `record` needs before it can run on `machine`, if it lacks one.
pub fn needs_engine(record: &ModelRecord, machine: &Machine) -> Option<EngineNeed> {
    if record.state == ModelState::Missing || record.downloading {
        return None;
    }
    match &record.runtime.id {
        Some(runtime) if *runtime == RuntimeId::llama_cpp() && !machine.engines.llama_cpp => {
            Some(EngineNeed::LlamaCpp)
        }
        None if record.source.kind == SourceKind::ollama() && !machine.engines.ollama => {
            Some(EngineNeed::Ollama)
        }
        _ => None,
    }
}

/// The build of `variant` (or the one that suits `machine`) the pin has for
/// this machine, or why there is none.
pub fn build_for(variant: Option<Variant>, machine: &Machine) -> Result<&'static Asset, CliError> {
    let arch = std::env::consts::ARCH;
    let os = Os::current();
    let chosen = match variant {
        Some(variant) => variant,
        None => Variant::default_for(machine).ok_or_else(|| {
            CliError::new(format!(
                "llama.cpp {} has no build for {} {arch}. {LLAMA_CPP_INSTALL_HINT}",
                PINNED.version,
                os.as_str()
            ))
        })?,
    };
    PINNED.asset(os, arch, chosen).ok_or_else(|| {
        let offered: Vec<&str> = PINNED
            .variants(os, arch)
            .into_iter()
            .map(Variant::as_str)
            .collect();
        CliError::new(format!(
            "llama.cpp {} has no {} build for {} {arch}; this machine has: {}",
            PINNED.version,
            chosen.as_str(),
            os.as_str(),
            if offered.is_empty() {
                "none".to_owned()
            } else {
                offered.join(", ")
            }
        ))
    })
}

/// The build of llama.cpp that suits `machine` better than `carried`, the
/// build hedos carries (the CUDA build on a Linux machine with an NVIDIA
/// card, say), when there is one to install.
pub fn faster_than(carried: Variant, machine: &Machine) -> Option<&'static Asset> {
    let suits = Variant::default_for(machine).filter(|suits| *suits != carried)?;
    PINNED.asset(machine.os, machine.arch.as_str(), suits)
}

/// One line saying how to install a faster llama.cpp than the one hedos
/// carries, when that is the one it runs and `machine` has a faster build.
pub fn faster_line(machine: &Machine) -> Option<String> {
    let carried = bundled()?.variant;
    located().filter(|server| server.from == Source::Bundled)?;
    published_builds_run_here().ok()?;
    let asset = faster_than(carried, machine)?;
    Some(format!(
        "hedos engines install llama.cpp fetches the {} build ({}), faster here than the {} build hedos carries",
        asset.variant.as_str(),
        format_bytes(i64::try_from(asset.download_bytes()).unwrap_or(i64::MAX)),
        carried.as_str()
    ))
}

/// Refuse, before any card or question, to install a build that would not
/// start on this system (llama.cpp's own Linux builds on an older glibc).
pub fn runs_here() -> Result<(), CliError> {
    published_builds_run_here().map_err(CliError::new)
}

/// What installing `asset` fetches, from where, how big, and where it goes,
/// one line each, for a person to say yes to.
pub fn card(asset: &Asset, dirs: &HedosDirs) -> Vec<String> {
    let mut lines = vec![
        format!(
            "llama.cpp {} ({}), the {} build",
            PINNED.version,
            PINNED.build,
            asset.variant.as_str()
        ),
        format!("  from     {}", PINNED.url(asset.file)),
        format!("  sha256   {}", asset.sha256),
    ];
    if let Some(runtime) = asset.cudart {
        lines.push(format!("  and      {}", PINNED.url(runtime.file)));
        lines.push(format!("  sha256   {}", runtime.sha256));
    }
    lines.push(format!(
        "  size     {}",
        format_bytes(i64::try_from(asset.download_bytes()).unwrap_or(i64::MAX))
    ));
    lines.push(format!("  into     {}", hedos_root(&dirs.data).display()));
    if matches!(asset.variant, Variant::Cuda12 | Variant::Cuda13)
        && let Some(vulkan) = PINNED.asset(asset.os, asset.arch, Variant::Vulkan)
    {
        lines.push(format!(
            "  smaller  --variant vulkan is {}, and slower on NVIDIA",
            format_bytes(i64::try_from(vulkan.download_bytes()).unwrap_or(i64::MAX))
        ));
    }
    lines
}

/// Install `asset`, showing how far it has got.
pub async fn install_build(
    asset: &Asset,
    dirs: &HedosDirs,
    out: &Out,
) -> Result<LlamaServer, CliError> {
    let animated = std::io::stderr().is_terminal();
    let spinner = Mutex::new(Spinner::start(out));
    let shown = Mutex::new(None::<u64>);
    let result = install(asset.variant, &dirs.data, |progress| {
        let message = match progress {
            InstallProgress::Downloading {
                received, total, ..
            } => {
                // A terminal redraws one line; a log gets a line per tenth.
                let step = if animated { 1 } else { 10 };
                let percent = received.saturating_mul(100) / total.max(1) / step * step;
                let Ok(mut shown) = shown.lock() else {
                    return;
                };
                if *shown == Some(percent) {
                    return;
                }
                *shown = Some(percent);
                format!(
                    "downloading llama.cpp {percent}% of {}",
                    format_bytes(i64::try_from(total).unwrap_or(i64::MAX))
                )
            }
            InstallProgress::Unpacking => "unpacking llama.cpp".to_owned(),
            InstallProgress::Checking => "checking llama.cpp runs".to_owned(),
        };
        if let Ok(mut spinner) = spinner.lock() {
            spinner.set(&message);
        }
    })
    .await;
    if let Ok(mut spinner) = spinner.lock() {
        spinner.clear();
    }
    result.map_err(|error| CliError::new(error.to_string()))
}

/// After `name` was pulled for llama.cpp: when no `llama-server` is there to
/// run it, ask once, on a terminal, whether to install the pinned build, and
/// install it on a yes. Under `--json`, off a terminal, or on a no, say how to
/// install it instead. Nothing is fetched without the yes.
pub async fn offer_after_pull(name: &str, machine: &Machine, out: &Out) -> Result<(), CliError> {
    if located().is_some() {
        return Ok(());
    }
    let Ok(asset) = build_for(None, machine) else {
        out.err(&format!(
            "{name} runs on llama.cpp, which isn't installed. {LLAMA_CPP_INSTALL_HINT}"
        ));
        return Ok(());
    };
    if let Err(refused) = runs_here() {
        out.err(&format!(
            "{name} runs on llama.cpp, which isn't installed. {refused}"
        ));
        return Ok(());
    }
    if !interactive::is_interactive(out) {
        out.err(&format!(
            "{name} runs on llama.cpp, which isn't installed. {LLAMA_CPP_INSTALL_HINT}"
        ));
        return Ok(());
    }
    let dirs = HedosDirs::detect();
    out.err(&format!("{name} runs on llama.cpp, which isn't installed."));
    for line in card(asset, &dirs) {
        out.err(&line);
    }
    if !interactive::confirm("Install llama.cpp now?", false)? {
        out.err(LLAMA_CPP_INSTALL_HINT);
        return Ok(());
    }
    let server = install_build(asset, &dirs, out).await?;
    out.line(&format!(
        "installed llama.cpp {} at {}",
        PINNED.version,
        server.path.display()
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::machine::DevicesFrom;

    fn machine(os: Os, arch: &str, devices_from: DevicesFrom) -> Machine {
        Machine {
            os,
            arch: arch.to_owned(),
            devices_from,
            ..Machine::default()
        }
    }

    #[test]
    fn an_nvidia_card_on_linux_has_a_faster_build_than_the_one_hedos_carries() {
        let nvidia = machine(Os::Linux, "x86_64", DevicesFrom::NvidiaSmi);
        assert_eq!(
            faster_than(Variant::Cpu, &nvidia).map(|asset| asset.variant),
            Some(Variant::Cuda12)
        );
        let plain = machine(Os::Linux, "x86_64", DevicesFrom::None);
        assert_eq!(faster_than(Variant::Cpu, &plain), None);
        let mac = machine(Os::Macos, "aarch64", DevicesFrom::None);
        assert_eq!(faster_than(Variant::Metal, &mac), None);
    }
}
