//! `hedos engines`: the engines models run on, where each is on this machine,
//! and installing or removing the ones hedos installs (llama.cpp, and the uv
//! the Python models' environments are built with), only when asked.

use clap::{Args, Subcommand};
use kernel::engines::llama_cpp::{
    DECISION_MODELS_BUILD, InstallSource, LlamaVersion, PINNED, Variant,
};
use kernel::engines::uv::PINNED as UV_PINNED;
use runtime::boot::HedosDirs;
use runtime::engines::bundled::bundled;
use runtime::engines::llama_cpp::{LlamaServer, Source, installed, located, on_path, version};
use runtime::engines::uv::{self, Outcome, Source as UvSource};
use runtime::engines::{ollama_binary, ollama_listening};

use crate::error::CliError;
use crate::support::engines::{
    Installable, build_for, card, consented, faster_line, install_build, install_pinned_uv,
    runs_here, uv_card, uv_for_here, uv_installed_line,
};
use crate::support::machine;
use crate::support::output::Out;
use crate::support::table;

/// Arguments for `engines`.
#[derive(Args)]
pub struct EnginesArgs {
    #[command(subcommand)]
    command: Option<EnginesCommand>,
}

#[derive(Subcommand)]
enum EnginesCommand {
    /// List the engines and where each is.
    Ls,
    /// Install llama.cpp or uv, after saying what will be fetched.
    Install(InstallArgs),
    /// Remove the llama.cpp builds or the uv hedos installed or unpacked.
    Remove(RemoveArgs),
}

#[derive(Args)]
struct InstallArgs {
    /// The engine: llama.cpp or uv.
    engine: String,
    /// llama.cpp's build: metal, cpu, vulkan, cuda-12, cuda-13 or rocm. The
    /// one that suits this machine when not given.
    #[arg(long)]
    variant: Option<String>,
    /// Install without being asked to confirm.
    #[arg(short, long)]
    yes: bool,
}

#[derive(Args)]
struct RemoveArgs {
    /// The engine: llama.cpp or uv.
    engine: String,
    /// Remove without being asked to confirm.
    #[arg(short, long)]
    yes: bool,
}

/// Run the `engines` command.
pub async fn run(args: EnginesArgs, out: &Out) -> Result<(), CliError> {
    match args.command.unwrap_or(EnginesCommand::Ls) {
        EnginesCommand::Ls => {
            list(out);
            Ok(())
        }
        EnginesCommand::Install(args) => match managed(&args.engine, Managing::Install)? {
            Installable::LlamaCpp => install(args, out).await,
            Installable::Uv => install_uv(args, out).await,
        },
        EnginesCommand::Remove(args) => match managed(&args.engine, Managing::Remove)? {
            Installable::LlamaCpp => remove(args, out),
            Installable::Uv => remove_uv(args, out),
        },
    }
}

/// One engine as this machine has it.
struct Row {
    name: &'static str,
    serves: &'static str,
    found: Option<String>,
    path: Option<String>,
    version: Option<String>,
    note: Option<String>,
}

fn rows() -> Vec<Row> {
    let llama = located();
    let machine = machine::machine();
    let dirs = HedosDirs::detect();
    // hedos's own builds say their version in the record beside the one
    // that runs, or for a carried build not unpacked yet, the pin; any other
    // is asked.
    let recorded = |server: &LlamaServer, source| {
        installed(&dirs.data, source)
            .filter(|(dir, _)| server.path.parent() == Some(dir.as_path()))
            .and_then(|(_, record)| {
                Some(LlamaVersion {
                    build: kernel::engines::llama_cpp::parse_build_tag(&record.build)?,
                    version: Some(record.version),
                })
            })
    };
    let unpacked = llama
        .as_ref()
        .is_some_and(|server| server.from != Source::Bundled || server.path.exists());
    let llama_version = llama.as_ref().and_then(|server| match server.from {
        Source::Hedos => recorded(server, InstallSource::Requested),
        Source::Bundled => recorded(server, InstallSource::Bundled).or_else(|| {
            Some(LlamaVersion {
                build: PINNED.build_number()?,
                version: Some(PINNED.version.to_owned()),
            })
        }),
        Source::Config | Source::Path => version(&server.path),
    });
    let old = llama_version
        .as_ref()
        .is_some_and(|version| version.build < DECISION_MODELS_BUILD);
    // One on the PATH that hedos does not run is still worth a word when it
    // is too old for what you might run it with yourself.
    let passed_over = on_path()
        .filter(|path| llama.as_ref().is_some_and(|server| server.path != *path))
        .and_then(|path| version(&path).map(|version| (path, version)))
        .filter(|(_, version)| version.build < DECISION_MODELS_BUILD)
        .map(|(path, version)| {
            format!(
                "{} on the PATH is {}, older than 0.6.0; hedos does not run it",
                path.display(),
                version.label()
            )
        });
    let llama_note = if llama.is_none() {
        Some("hedos engines install llama.cpp".to_owned())
    } else {
        let notes: Vec<String> = [
            old.then(|| "older than 0.6.0, which decision models need".to_owned()),
            (!unpacked).then(|| "unpacks the first time a model runs".to_owned()),
            passed_over,
            faster_line(&machine),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!notes.is_empty()).then(|| notes.join("; "))
    };
    let ollama = ollama_binary();
    let listening = ollama_listening();
    let uv = uv::located();
    let uv_version = uv.as_ref().and_then(|uv| match uv.from {
        UvSource::Hedos => uv::installed(&dirs.data).map(|(_, record)| record.version),
        UvSource::Path => uv::version(&uv.path),
    });
    vec![
        Row {
            name: "llama.cpp",
            serves: "GGUF models",
            found: llama.as_ref().map(|server| server.from.as_str().to_owned()),
            path: llama
                .as_ref()
                .map(|server| server.path.display().to_string()),
            version: llama_version.as_ref().map(|version| version.label()),
            note: llama_note,
        },
        Row {
            name: "Ollama",
            serves: "models in Ollama's store",
            found: match (&ollama, listening) {
                (Some(_), _) => Some("path".to_owned()),
                (None, true) => Some("daemon".to_owned()),
                (None, false) => None,
            },
            path: ollama.as_ref().map(|path| path.display().to_string()),
            version: None,
            note: (ollama.is_none() && !listening)
                .then(|| "optional; get it from ollama.com".to_owned()),
        },
        Row {
            name: "uv",
            serves: "Python models",
            found: uv.as_ref().map(|uv| uv.from.as_str().to_owned()),
            path: uv.as_ref().map(|uv| uv.path.display().to_string()),
            version: uv_version,
            note: uv
                .is_none()
                .then(|| "hedos engines install uv, for Python models".to_owned()),
        },
    ]
}

fn list(out: &Out) {
    let rows = rows();
    out.json(&serde_json::json!({
        "engines": rows
            .iter()
            .map(|row| serde_json::json!({
                "engine": row.name,
                "serves": row.serves,
                "installed": row.found.is_some(),
                "found_in": row.found,
                "path": row.path,
                "version": row.version,
                "note": row.note,
            }))
            .collect::<Vec<_>>(),
    }));
    let dash = || "-".to_owned();
    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.name.to_owned(),
                row.serves.to_owned(),
                row.found.clone().unwrap_or_else(|| "missing".to_owned()),
                row.version.clone().unwrap_or_else(dash),
                row.note
                    .clone()
                    .or_else(|| row.path.clone())
                    .unwrap_or_else(dash),
            ]
        })
        .collect();
    out.line(&table::render(
        &["ENGINE", "SERVES", "FOUND", "VERSION", "PATH OR NEXT STEP"],
        &table_rows,
    ));
}

/// What `engines install` or `engines remove` does to an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Managing {
    Install,
    Remove,
}

/// The engine `engine` names, when hedos installs it; otherwise refused, for
/// an install saying where it comes from, for a removal that hedos removes
/// only what it installed.
fn managed(engine: &str, managing: Managing) -> Result<Installable, CliError> {
    match engine.to_ascii_lowercase().as_str() {
        "llama.cpp" | "llama-cpp" | "llamacpp" | "llama-server" => Ok(Installable::LlamaCpp),
        "uv" => Ok(Installable::Uv),
        "ollama" => Err(CliError::new(match managing {
            Managing::Install => "hedos doesn't install Ollama; get it from ollama.com",
            Managing::Remove => {
                "hedos doesn't remove Ollama; it removes only the llama.cpp and uv it installed or unpacked"
            }
        })),
        other => {
            let verb = match managing {
                Managing::Install => "installs",
                Managing::Remove => "removes",
            };
            Err(CliError::new(format!(
                "no engine named {other}; llama.cpp and uv are the ones hedos {verb}"
            )))
        }
    }
}

async fn install(args: InstallArgs, out: &Out) -> Result<(), CliError> {
    let variant = match &args.variant {
        Some(name) => Some(Variant::parse(name).ok_or_else(|| {
            CliError::new(format!(
                "no build named {name}; use metal, cpu, vulkan, cuda-12, cuda-13 or rocm"
            ))
        })?),
        None => None,
    };
    let machine = machine::machine();
    let asset = build_for(variant, &machine)?;
    // Only when the carried build is the one that runs: a build installed on
    // request or named in the config wins over it, and installing is how
    // that one is repaired or replaced.
    if let Some(carried) = carried_answers(asset.variant) {
        // Only builds that start here: on an older Linux llama.cpp's own
        // builds do not.
        let others: Vec<&str> = PINNED
            .variants_starting(asset.os, asset.arch, runtime::engines::glibc().as_deref())
            .into_iter()
            .filter(|other| *other != carried.variant)
            .map(Variant::as_str)
            .collect();
        out.line(&format!(
            "hedos carries llama.cpp {} ({}), the {} build: nothing to install",
            PINNED.version,
            PINNED.build,
            carried.variant.as_str()
        ));
        if !others.is_empty() {
            out.err(&format!(
                "other builds for this machine: --variant {}",
                others.join(", --variant ")
            ));
        }
        out.json(&install_json(
            carried.variant,
            located().map(|server| server.path),
            "carried",
        ));
        return Ok(());
    }
    runs_here(asset)?;
    let dirs = HedosDirs::detect();
    for line in card(asset, &dirs) {
        out.err(&line);
    }
    if !consented(args.yes, out, "installing llama.cpp", "Install llama.cpp?")? {
        out.line("Not installed.");
        return Ok(());
    }
    let server = install_build(asset, &dirs, out).await?;
    out.line(&format!(
        "installed llama.cpp {} at {}",
        PINNED.version,
        server.path.display()
    ));
    let runs = located();
    if let Some(other) = runs.as_ref().filter(|runs| runs.from == Source::Config) {
        out.err(&format!(
            "`[engines] llama_server` names {}, which hedos runs ahead of this one",
            other.path.display()
        ));
    }
    out.json(&install_json(asset.variant, Some(server.path), "installed"));
    Ok(())
}

/// What `engines install --json` prints, one shape for both outcomes:
/// `status` is `installed` when a build was fetched now, `carried` when the
/// build hedos carries is the one asked for and nothing was.
fn install_json(
    variant: Variant,
    path: Option<std::path::PathBuf>,
    status: &str,
) -> serde_json::Value {
    serde_json::json!({
        "engine": "llama.cpp",
        "version": PINNED.version,
        "build": PINNED.build,
        "variant": variant.as_str(),
        "path": path,
        "status": status,
    })
}

/// The build hedos carries, when it is the `variant` asked for and the one
/// that runs.
fn carried_answers(variant: Variant) -> Option<runtime::engines::bundled::Bundled> {
    bundled()
        .filter(|carried| carried.variant == variant)
        .filter(|_| located().is_some_and(|server| server.from == Source::Bundled))
}

fn remove(args: RemoveArgs, out: &Out) -> Result<(), CliError> {
    let dirs = HedosDirs::detect();
    let asked = if bundled().is_some() {
        "Remove the llama.cpp builds hedos installed or unpacked? The one it carries unpacks again when needed."
    } else {
        "Remove the llama.cpp hedos installed?"
    };
    if !consented(args.yes, out, "removing llama.cpp", asked)? {
        out.line("Kept.");
        return Ok(());
    }
    let removed = runtime::engines::install::remove(&dirs.data)
        .map_err(|error| CliError::new(error.to_string()))?;
    if removed.is_empty() {
        out.line("hedos has no llama.cpp installed or unpacked to remove.");
    } else {
        out.line("removed the llama.cpp builds hedos installed or unpacked");
    }
    let now = located();
    match &now {
        Some(server) if server.from == Source::Bundled => out.err(
            "hedos now runs the llama.cpp it carries, which unpacks the first time a model runs",
        ),
        Some(server) => out.err(&format!(
            "hedos now runs the llama-server at {}",
            server.path.display()
        )),
        None => out.err("no llama-server is left to run GGUF models"),
    }
    out.json(&serde_json::json!({
        "engine": "llama.cpp",
        "removed": removed,
        "now_runs": now.map(|server| server.path),
    }));
    Ok(())
}

async fn install_uv(args: InstallArgs, out: &Out) -> Result<(), CliError> {
    install_uv_in(args, &HedosDirs::detect(), out).await
}

/// [`install_uv`] under `dirs`.
async fn install_uv_in(args: InstallArgs, dirs: &HedosDirs, out: &Out) -> Result<(), CliError> {
    if args.variant.is_some() {
        return Err(CliError::new(
            "--variant is llama.cpp's; uv has one build for each machine",
        ));
    }
    let asset = uv_for_here()?;
    if let Some(present) = uv::present(&dirs.data) {
        out.line(&uv_installed_line(&present, Outcome::Present));
        out.json(&uv_json(Outcome::Present, &present.path));
        return Ok(());
    }
    for line in uv_card(asset, dirs) {
        out.err(&line);
    }
    if !consented(args.yes, out, "installing uv", "Install uv?")? {
        out.line("Not installed.");
        return Ok(());
    }
    let (installed, outcome) = install_pinned_uv(dirs, out).await?;
    out.line(&uv_installed_line(&installed, outcome));
    out.json(&uv_json(outcome, &installed.path));
    Ok(())
}

/// What `engines install uv --json` prints: `status` is `installed` when it
/// was fetched now, `present` when the pinned uv was already there.
fn uv_json(outcome: Outcome, path: &std::path::Path) -> serde_json::Value {
    let status = match outcome {
        Outcome::Installed => "installed",
        Outcome::Present => "present",
    };
    serde_json::json!({
        "engine": "uv",
        "version": UV_PINNED.version,
        "path": path,
        "status": status,
    })
}

fn remove_uv(args: RemoveArgs, out: &Out) -> Result<(), CliError> {
    let dirs = HedosDirs::detect();
    let asked = "Remove the uv hedos installed? Models whose environments are built keep running; one being prepared now may fail.";
    if !consented(args.yes, out, "removing uv", asked)? {
        out.line("Kept.");
        return Ok(());
    }
    let removed = uv::remove(&dirs.data).map_err(|error| CliError::new(error.to_string()))?;
    if removed.is_empty() {
        out.line("hedos has no uv installed to remove.");
    } else {
        out.line("removed the uv hedos installed");
    }
    let now = uv::located();
    match &now {
        Some(uv) => out.err(&format!(
            "hedos now builds Python environments with the uv at {}",
            uv.path.display()
        )),
        None => out.err(
            "no uv is left; Python models whose environments are built still run, others need `hedos engines install uv`",
        ),
    }
    out.json(&serde_json::json!({
        "engine": "uv",
        "removed": removed,
        "now_runs": now.map(|uv| uv.path),
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_cpp_and_uv_are_installed_and_ollama_says_where_to_get_it() {
        for name in ["llama.cpp", "LLAMA-CPP", "llama-server"] {
            assert_eq!(
                managed(name, Managing::Install).ok(),
                Some(Installable::LlamaCpp)
            );
            assert_eq!(
                managed(name, Managing::Remove).ok(),
                Some(Installable::LlamaCpp)
            );
        }
        for managing in [Managing::Install, Managing::Remove] {
            assert_eq!(managed("UV", managing).ok(), Some(Installable::Uv));
        }
        let said = |engine, managing| managed(engine, managing).unwrap_err().to_string();
        assert!(said("ollama", Managing::Install).contains("ollama.com"));
        assert!(said("vllm", Managing::Install).ends_with("hedos installs"));
        // A removal says what hedos removes, not where to get the engine.
        let removing = said("ollama", Managing::Remove);
        assert!(
            removing.starts_with("hedos doesn't remove Ollama") && !removing.contains("get it"),
            "{removing}"
        );
        assert!(said("vllm", Managing::Remove).ends_with("hedos removes"));
    }

    #[tokio::test]
    async fn installing_without_a_yes_off_a_terminal_fetches_nothing() {
        let json = Out::new(true);
        let asked = install(
            InstallArgs {
                engine: "llama.cpp".to_owned(),
                variant: None,
                yes: false,
            },
            &json,
        )
        .await;
        // A build that carries the one this machine would get has nothing to
        // install; any other asks first.
        let carried = runtime::engines::variant_for(&machine::machine())
            .and_then(carried_answers)
            .is_some();
        match asked {
            Ok(()) => assert!(carried, "installed without a yes"),
            Err(refused) => {
                assert!(!carried, "{refused}");
                assert!(refused.to_string().contains("needs a yes"), "{refused}");
            }
        }

        let unknown = install(
            InstallArgs {
                engine: "llama.cpp".to_owned(),
                variant: Some("opencl".to_owned()),
                yes: true,
            },
            &json,
        )
        .await
        .unwrap_err();
        assert!(
            unknown.to_string().contains("no build named opencl"),
            "{unknown}"
        );
    }

    #[tokio::test]
    async fn uv_takes_no_variant_and_installs_only_on_a_yes() {
        let json = Out::new(true);
        let refused = install_uv(
            InstallArgs {
                engine: "uv".to_owned(),
                variant: Some("cpu".to_owned()),
                yes: true,
            },
            &json,
        )
        .await
        .unwrap_err();
        assert!(refused.to_string().contains("--variant"), "{refused}");

        // A data dir with no uv of hedos's asks first, and off a terminal
        // fetches nothing.
        let scratch = std::env::temp_dir().join(format!(
            "hedos-cli-uv-{}-{}",
            std::process::id(),
            kernel::time::now_millis()
        ));
        let dirs = HedosDirs {
            data: scratch.clone(),
        };
        let ask = || InstallArgs {
            engine: "uv".to_owned(),
            variant: None,
            yes: false,
        };
        let refused = install_uv_in(ask(), &dirs, &json).await.unwrap_err();
        assert!(refused.to_string().contains("needs a yes"), "{refused}");
        assert!(uv::installed(&dirs.data).is_none());

        // With the pinned uv there, whole, there is nothing to ask.
        let asset = uv_for_here().expect("a build for this machine");
        let dir = uv::hedos_root(&dirs.data).join(UV_PINNED.version);
        std::fs::create_dir_all(&dir).unwrap();
        // It answers with the version pinned, as `present` asks it to.
        let program = format!("#!/bin/sh\necho 'uv {} (test)'\n", UV_PINNED.version);
        std::fs::write(dir.join("uv"), &program).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("uv"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let record = kernel::engines::uv::Installed {
            version: UV_PINNED.version.to_owned(),
            os: asset.os.as_str().to_owned(),
            arch: asset.arch.to_owned(),
            sha256: asset.sha256.to_owned(),
            installed_at: 1,
            files: vec![kernel::engines::InstalledFile {
                name: "uv".to_owned(),
                bytes: Some(program.len() as u64),
            }],
        };
        std::fs::write(
            dir.join(kernel::engines::INSTALLED_FILE),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        install_uv_in(ask(), &dirs, &json)
            .await
            .expect("nothing to install");
        let _ = std::fs::remove_dir_all(&scratch);

        let refused = remove_uv(
            RemoveArgs {
                engine: "uv".to_owned(),
                yes: false,
            },
            &json,
        )
        .unwrap_err();
        assert!(refused.to_string().contains("needs a yes"), "{refused}");
    }

    #[test]
    fn removing_without_a_yes_off_a_terminal_removes_nothing() {
        let refused = remove(
            RemoveArgs {
                engine: "llama.cpp".to_owned(),
                yes: false,
            },
            &Out::new(true),
        )
        .unwrap_err();
        assert!(refused.to_string().contains("needs a yes"), "{refused}");
    }

    #[test]
    fn every_engine_is_listed_with_what_it_serves() {
        let rows = rows();
        let names: Vec<&str> = rows.iter().map(|row| row.name).collect();
        assert_eq!(names, ["llama.cpp", "Ollama", "uv"]);
        assert!(rows.iter().all(|row| !row.serves.is_empty()));
        assert!(
            rows.iter()
                .all(|row| row.found.is_some() || row.note.is_some())
        );
    }
}
