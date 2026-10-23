//! Installing an engine from the CLI (llama.cpp, or the uv the Python models'
//! environments are built with), the same way from `hedos engines install`
//! and from the question `hedos pull` asks when a model it pulled needs it:
//! say what will be fetched, from where, how big and where it goes, and fetch
//! it only on a yes.

use std::collections::HashSet;
use std::io::IsTerminal;
use std::sync::Mutex;

use kernel::engines::llama_cpp::{Asset, PINNED, Variant};
use kernel::engines::uv::{Asset as UvAsset, PINNED as UV_PINNED};
use kernel::machine::{LLAMA_CPP_INSTALL_HINT, Machine, Os, UV_INSTALL_HINT};
use kernel::records::{ModelRecord, ModelState, RuntimeId, SourceKind, format_bytes};
use runtime::boot::HedosDirs;
use runtime::engines::bundled::bundled;
use runtime::engines::install::{InstallProgress, Landing, install};
use runtime::engines::llama_cpp::{LlamaServer, Source, hedos_root, located};
use runtime::engines::uv::{self, Outcome, Source as UvSource, Uv};
use runtime::engines::{glibc, variant_for};

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::spinner::Spinner;

/// An engine hedos installs, and removes, when a person asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Installable {
    /// llama.cpp's `llama-server`, which serves GGUF models.
    LlamaCpp,
    /// uv, which builds the Python models' environments.
    Uv,
}

impl Installable {
    /// The engine as commands and messages name it.
    pub fn name(self) -> &'static str {
        match self {
            Installable::LlamaCpp => "llama.cpp",
            Installable::Uv => "uv",
        }
    }
}

/// An engine a model on the shelf needs and this machine lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineNeed {
    /// One hedos installs when asked: llama.cpp when the model resolved to it
    /// and no `llama-server` is here, uv when its runtime's Python environment
    /// is not built and no uv is here to build it.
    Install(Installable),
    /// It is in Ollama's own format, which only Ollama runs, and Ollama is
    /// not here: nothing serves it.
    Ollama,
}

/// The engine `record` needs before it can run on `machine`, if it lacks one;
/// `needs_uv` holds the runtimes that need uv now ([`runtimes_needing_uv`]).
pub fn needs_engine(
    record: &ModelRecord,
    machine: &Machine,
    needs_uv: &HashSet<RuntimeId>,
) -> Option<EngineNeed> {
    if record.state == ModelState::Missing || record.downloading {
        return None;
    }
    match &record.runtime.id {
        Some(runtime) if *runtime == RuntimeId::llama_cpp() && !machine.engines.llama_cpp => {
            Some(EngineNeed::Install(Installable::LlamaCpp))
        }
        Some(runtime) if needs_uv.contains(runtime) => Some(EngineNeed::Install(Installable::Uv)),
        None if record.source.kind == SourceKind::ollama() && !machine.engines.ollama => {
            Some(EngineNeed::Ollama)
        }
        _ => None,
    }
}

/// The runtimes of `records` that need uv before their models run: none when
/// a uv is here, else each whose Python environment is not built.
pub fn runtimes_needing_uv(records: &[ModelRecord]) -> HashSet<RuntimeId> {
    let runtimes: HashSet<&RuntimeId> = records
        .iter()
        .filter_map(|record| record.runtime.id.as_ref())
        .collect();
    uv::needing(runtimes)
}

/// The build of `variant` (or the one that suits `machine`) the pin has for
/// this machine, or why there is none.
pub fn build_for(variant: Option<Variant>, machine: &Machine) -> Result<&'static Asset, CliError> {
    let arch = std::env::consts::ARCH;
    let os = Os::current();
    let chosen = match variant {
        Some(variant) => variant,
        None => variant_for(machine).ok_or_else(|| {
            CliError::new(format!(
                "llama.cpp {} has no build for {} {arch}. {LLAMA_CPP_INSTALL_HINT}",
                PINNED.version,
                os.as_str()
            ))
        })?,
    };
    PINNED.asset(os, arch, chosen).ok_or_else(|| {
        let offered: Vec<&str> = PINNED
            .variants_starting(os, arch, glibc().as_deref())
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

/// The build of llama.cpp that suits `machine`, whose glibc is `glibc`,
/// better than `carried`, the build hedos carries (the CUDA build on a Linux
/// machine with an NVIDIA card, say), when there is one to install.
pub fn faster_than(
    carried: Variant,
    machine: &Machine,
    glibc: Option<&str>,
) -> Option<&'static Asset> {
    let suits = Variant::default_for(machine, glibc).filter(|suits| *suits != carried)?;
    PINNED.asset(machine.os, machine.arch.as_str(), suits)
}

/// One line saying how to install a faster llama.cpp than the one hedos
/// carries, when that is the one it runs and `machine` has a faster build.
pub fn faster_line(machine: &Machine) -> Option<String> {
    let carried = bundled()?.variant;
    located().filter(|server| server.from == Source::Bundled)?;
    let asset = faster_than(carried, machine, glibc().as_deref())?;
    Some(format!(
        "hedos engines install llama.cpp fetches the {} build ({}), faster here than the {} build hedos carries",
        asset.variant.as_str(),
        format_bytes(i64::try_from(asset.download_bytes()).unwrap_or(i64::MAX)),
        carried.as_str()
    ))
}

/// Whether the person running a command said yes to `question`: `--yes`
/// (`yes`) answers it for them; off a terminal or under `--json` nobody can
/// be asked, so `doing` (`installing uv`) is refused instead. The question
/// defaults to no.
pub fn consented(yes: bool, out: &Out, doing: &str, question: &str) -> Result<bool, CliError> {
    if yes {
        return Ok(true);
    }
    if !interactive::is_interactive(out) {
        return Err(CliError::new(format!(
            "{doing} needs a yes: run this in a terminal, or pass --yes"
        )));
    }
    interactive::confirm(question, false)
}

/// Refuse, before any card or question, to install a build that would not
/// start on this system (llama.cpp's own Linux builds on an older glibc).
pub fn runs_here(asset: &Asset) -> Result<(), CliError> {
    runtime::engines::runs_here(asset).map_err(|refusal| CliError::new(refusal.to_string()))
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
        format!("  from     {}", PINNED.url(asset.publisher, asset.file)),
        format!("  sha256   {}", asset.sha256),
    ];
    if let Some(runtime) = asset.cudart {
        lines.push(format!(
            "  and      {}",
            PINNED.url(asset.publisher, runtime.file)
        ));
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

/// How far an install has got, shown as it goes: one line a terminal redraws,
/// or a line per tenth for a log.
struct Shown {
    engine: &'static str,
    animated: bool,
    spinner: Mutex<Spinner>,
    percent: Mutex<Option<u64>>,
}

impl Shown {
    fn start(engine: &'static str, out: &Out) -> Self {
        Self {
            engine,
            animated: std::io::stderr().is_terminal(),
            spinner: Mutex::new(Spinner::start(out)),
            percent: Mutex::new(None),
        }
    }

    fn report(&self, progress: InstallProgress) {
        let engine = self.engine;
        let message = match progress {
            InstallProgress::Downloading {
                received, total, ..
            } => {
                let step = if self.animated { 1 } else { 10 };
                let percent = received.saturating_mul(100) / total.max(1) / step * step;
                let Ok(mut shown) = self.percent.lock() else {
                    return;
                };
                if *shown == Some(percent) {
                    return;
                }
                *shown = Some(percent);
                format!(
                    "downloading {engine} {percent}% of {}",
                    format_bytes(i64::try_from(total).unwrap_or(i64::MAX))
                )
            }
            InstallProgress::Unpacking => format!("unpacking {engine}"),
            InstallProgress::Checking => format!("checking {engine} runs"),
        };
        if let Ok(mut spinner) = self.spinner.lock() {
            spinner.set(&message);
        }
    }

    /// Wait for `installing`, which Ctrl-C stops, whatever handler a command
    /// before it (a pull it followed) left installed; what a stopped install
    /// leaves, the next one puts right. A Ctrl-C that came as it was already
    /// being put in place (dropping it waits for that) is not called a stop:
    /// `landed` says what it put there.
    async fn follow<T, E: std::fmt::Display>(
        &self,
        installing: impl std::future::Future<Output = Result<T, E>>,
        landed: impl FnOnce() -> Option<T>,
    ) -> Result<T, CliError> {
        self.follow_until(
            installing,
            crate::support::signals::wait_for_ctrl_c(),
            landed,
        )
        .await
    }

    /// [`follow`](Self::follow), stopped by `stop`.
    async fn follow_until<T, E: std::fmt::Display>(
        &self,
        installing: impl std::future::Future<Output = Result<T, E>>,
        stop: impl std::future::Future<Output = ()>,
        landed: impl FnOnce() -> Option<T>,
    ) -> Result<T, CliError> {
        let engine = self.engine;
        // An install that has finished wins over a stop that came with it.
        let finished = tokio::select! {
            biased;
            result = installing => Some(result),
            () = stop => None,
        };
        let result = match finished {
            Some(result) => result.map_err(|error| CliError::new(error.to_string())),
            None => landed().ok_or_else(|| {
                CliError::new(format!(
                    "stopped installing {engine}; hedos engines install {engine} starts it again"
                ))
            }),
        };
        if let Ok(mut spinner) = self.spinner.lock() {
            spinner.clear();
        }
        result
    }
}

/// Install `asset`, showing how far it has got.
pub async fn install_build(
    asset: &Asset,
    dirs: &HedosDirs,
    out: &Out,
) -> Result<LlamaServer, CliError> {
    let shown = Shown::start("llama.cpp", out);
    let landing = Landing::default();
    let landed = || {
        landing.landed().map(|dir| LlamaServer {
            path: dir.join("llama-server"),
            from: Source::Hedos,
        })
    };
    shown
        .follow(
            install(asset.variant, &dirs.data, &landing, |progress| {
                shown.report(progress)
            }),
            landed,
        )
        .await
}

/// What installing uv fetches, from where, how big, where it goes, and what
/// the first Python model to run fetches after it, one line each, for a
/// person to say yes to.
pub fn uv_card(asset: &UvAsset, dirs: &HedosDirs) -> Vec<String> {
    vec![
        format!("uv {}", UV_PINNED.version),
        format!("  from     {}", UV_PINNED.url(asset.file)),
        format!("  sha256   {}", asset.sha256),
        format!(
            "  size     {}",
            format_bytes(i64::try_from(asset.bytes).unwrap_or(i64::MAX))
        ),
        format!("  into     {}", uv::hedos_root(&dirs.data).display()),
        "  then     a Python model's first run fetches its packages, and Python 3.12 if missing"
            .to_owned(),
    ]
}

/// The uv this hedos pins for this machine, or why there is none and what
/// to do instead: hedos cannot install it here, so only a uv put on PATH
/// builds the environments.
pub fn uv_for_here() -> Result<&'static UvAsset, CliError> {
    let arch = std::env::consts::ARCH;
    let os = Os::current();
    UV_PINNED.asset(os, arch).ok_or_else(|| {
        CliError::new(format!(
            "uv {} has no build for {} {arch}; put a `uv` on PATH (astral.sh/uv)",
            UV_PINNED.version,
            os.as_str()
        ))
    })
}

/// Install the pinned uv, showing how far it has got.
pub async fn install_pinned_uv(dirs: &HedosDirs, out: &Out) -> Result<(Uv, Outcome), CliError> {
    let shown = Shown::start("uv", out);
    let landing = Landing::default();
    let landed = || {
        landing.landed().map(|dir| {
            let uv = Uv {
                path: dir.join("uv"),
                from: UvSource::Hedos,
            };
            (uv, Outcome::Installed)
        })
    };
    shown
        .follow(
            uv::install(&dirs.data, &landing, |progress| shown.report(progress)),
            landed,
        )
        .await
}

/// What a uv install did, said in one line: one that found the pinned uv
/// already there (another install finished first) says so rather than
/// claiming to have fetched it.
pub fn uv_installed_line(uv: &Uv, outcome: Outcome) -> String {
    match outcome {
        Outcome::Installed => format!(
            "installed uv {} at {}",
            UV_PINNED.version,
            uv.path.display()
        ),
        Outcome::Present => format!(
            "uv {} is installed at {}: nothing to install",
            UV_PINNED.version,
            uv.path.display()
        ),
    }
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
            "{name} runs on llama.cpp. {LLAMA_CPP_INSTALL_HINT}"
        ));
        return Ok(());
    };
    if let Err(refused) = runs_here(asset) {
        out.err(&format!(
            "{name} runs on llama.cpp, which isn't installed. {refused}"
        ));
        return Ok(());
    }
    if !interactive::is_interactive(out) {
        out.err(&format!(
            "{name} runs on llama.cpp. {LLAMA_CPP_INSTALL_HINT}"
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

/// After `name` was pulled for a runtime whose Python environment is not
/// built, with no uv here: ask once, on a terminal, whether to install the
/// pinned uv, and install it on a yes. Under `--json`, off a terminal, or on
/// a no, say how to install it instead. Nothing is fetched without the yes.
pub async fn offer_uv_after_pull(name: &str, out: &Out) -> Result<(), CliError> {
    let needs = format!("{name} runs in a Python environment that uv builds.");
    let asset = match uv_for_here() {
        Ok(asset) => asset,
        Err(none) => {
            out.err(&format!("{needs} {none}"));
            return Ok(());
        }
    };
    if !interactive::is_interactive(out) {
        out.err(&format!("{needs} {UV_INSTALL_HINT}"));
        return Ok(());
    }
    let dirs = HedosDirs::detect();
    out.err(&format!("{needs} uv isn't installed."));
    for line in uv_card(asset, &dirs) {
        out.err(&line);
    }
    if !interactive::confirm("Install uv now?", false)? {
        out.err(UV_INSTALL_HINT);
        return Ok(());
    }
    let (installed, outcome) = install_pinned_uv(&dirs, out).await?;
    out.line(&uv_installed_line(&installed, outcome));
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

    #[tokio::test]
    async fn an_install_that_finished_wins_over_a_stop_that_came_with_it() {
        let shown = Shown::start("uv", &Out::new(true));
        for _ in 0..32 {
            let finished = shown
                .follow_until(
                    std::future::ready(Ok::<_, String>("present")),
                    std::future::ready(()),
                    || None,
                )
                .await;
            assert_eq!(finished.ok(), Some("present"));
        }
        let stopped = shown
            .follow_until(
                std::future::pending::<Result<&str, String>>(),
                std::future::ready(()),
                || Some("landed"),
            )
            .await;
        assert_eq!(stopped.ok(), Some("landed"));
        let stopped = shown
            .follow_until(
                std::future::pending::<Result<&str, String>>(),
                std::future::ready(()),
                || None,
            )
            .await;
        assert!(stopped.is_err_and(|error| error.to_string().starts_with("stopped installing uv")));
    }

    #[test]
    fn an_nvidia_card_on_linux_has_a_faster_build_than_the_one_hedos_carries() {
        let nvidia = machine(Os::Linux, "x86_64", DevicesFrom::NvidiaSmi);
        assert_eq!(
            faster_than(Variant::Cpu, &nvidia, None).map(|asset| asset.variant),
            Some(Variant::Cuda12)
        );
        // On a glibc too old for llama.cpp's CUDA build, the CPU build hedos
        // carries is the one that suits.
        assert_eq!(faster_than(Variant::Cpu, &nvidia, Some("2.35")), None);
        let plain = machine(Os::Linux, "x86_64", DevicesFrom::None);
        assert_eq!(faster_than(Variant::Cpu, &plain, None), None);
        let mac = machine(Os::Macos, "aarch64", DevicesFrom::None);
        assert_eq!(faster_than(Variant::Metal, &mac, None), None);
    }
}
