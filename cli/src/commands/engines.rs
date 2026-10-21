//! `hedos engines`: the engines models run on, where each is on this machine,
//! and installing or removing llama.cpp, the one hedos installs, only when
//! asked.

use clap::{Args, Subcommand};
use kernel::engines::llama_cpp::{
    DECISION_MODELS_BUILD, InstallSource, LlamaVersion, PINNED, Variant,
};
use runtime::boot::HedosDirs;
use runtime::engines::bundled::bundled;
use runtime::engines::llama_cpp::{LlamaServer, Source, installed, located, on_path, version};
use runtime::engines::{ollama_binary, ollama_listening, uv_binary};

use crate::error::CliError;
use crate::support::engines::{build_for, card, faster_line, install_build, runs_here};
use crate::support::interactive;
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
    /// Install llama.cpp, after saying what will be fetched.
    Install(InstallArgs),
    /// Remove the llama.cpp builds hedos installed or unpacked.
    Remove(RemoveArgs),
}

#[derive(Args)]
struct InstallArgs {
    /// The engine: llama.cpp is the one hedos installs.
    engine: String,
    /// The build: metal, cpu, vulkan, cuda-12, cuda-13 or rocm. The one that
    /// suits this machine when not given.
    #[arg(long)]
    variant: Option<String>,
    /// Install without being asked to confirm.
    #[arg(short, long)]
    yes: bool,
}

#[derive(Args)]
struct RemoveArgs {
    /// The engine: llama.cpp.
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
        EnginesCommand::Install(args) => install(args, out).await,
        EnginesCommand::Remove(args) => remove(args, out),
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
                    build: record.build.strip_prefix('b')?.parse().ok()?,
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
    let uv = uv_binary();
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
            found: uv.as_ref().map(|_| "path".to_owned()),
            path: uv.as_ref().map(|path| path.display().to_string()),
            version: None,
            note: uv
                .is_none()
                .then(|| "only for Python models; get it from astral.sh/uv".to_owned()),
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

/// Refuse any engine but llama.cpp, saying where the others come from.
fn llama_cpp_only(engine: &str) -> Result<(), CliError> {
    match engine.to_ascii_lowercase().as_str() {
        "llama.cpp" | "llama-cpp" | "llamacpp" | "llama-server" => Ok(()),
        "ollama" => Err(CliError::new(
            "hedos doesn't install Ollama; get it from ollama.com",
        )),
        "uv" => Err(CliError::new(
            "hedos doesn't install uv; get it from astral.sh/uv",
        )),
        other => Err(CliError::new(format!(
            "no engine named {other}; llama.cpp is the one hedos installs"
        ))),
    }
}

async fn install(args: InstallArgs, out: &Out) -> Result<(), CliError> {
    llama_cpp_only(&args.engine)?;
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
        out.json(&serde_json::json!({
            "engine": "llama.cpp",
            "version": PINNED.version,
            "build": PINNED.build,
            "variant": carried.variant.as_str(),
            "installed": false,
            "carried": true,
        }));
        return Ok(());
    }
    runs_here(asset)?;
    let dirs = HedosDirs::detect();
    for line in card(asset, &dirs) {
        out.err(&line);
    }
    if !args.yes {
        if !interactive::is_interactive(out) {
            return Err(CliError::new(
                "installing llama.cpp needs a yes: run this in a terminal, or pass --yes",
            ));
        }
        if !interactive::confirm("Install llama.cpp?", false)? {
            out.line("Not installed.");
            return Ok(());
        }
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
    out.json(&serde_json::json!({
        "engine": "llama.cpp",
        "version": PINNED.version,
        "build": PINNED.build,
        "variant": asset.variant.as_str(),
        "path": server.path,
        "installed": true,
    }));
    Ok(())
}

/// The build hedos carries, when it is the `variant` asked for and the one
/// that runs.
fn carried_answers(variant: Variant) -> Option<runtime::engines::bundled::Bundled> {
    bundled()
        .filter(|carried| carried.variant == variant)
        .filter(|_| located().is_some_and(|server| server.from == Source::Bundled))
}

fn remove(args: RemoveArgs, out: &Out) -> Result<(), CliError> {
    llama_cpp_only(&args.engine)?;
    let dirs = HedosDirs::detect();
    if !args.yes {
        if !interactive::is_interactive(out) {
            return Err(CliError::new(
                "removing llama.cpp needs a yes: run this in a terminal, or pass --yes",
            ));
        }
        let asked = if bundled().is_some() {
            "Remove the llama.cpp builds hedos installed or unpacked? The one it carries unpacks again when needed."
        } else {
            "Remove the llama.cpp hedos installed?"
        };
        if !interactive::confirm(asked, false)? {
            out.line("Kept.");
            return Ok(());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_llama_cpp_is_installed_and_the_others_say_where_to_get_them() {
        for name in ["llama.cpp", "LLAMA-CPP", "llama-server"] {
            assert!(llama_cpp_only(name).is_ok(), "{name}");
        }
        assert!(
            llama_cpp_only("ollama")
                .unwrap_err()
                .to_string()
                .contains("ollama.com")
        );
        assert!(
            llama_cpp_only("uv")
                .unwrap_err()
                .to_string()
                .contains("astral.sh/uv")
        );
        assert!(llama_cpp_only("vllm").is_err());
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
