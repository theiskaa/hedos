//! `hedos recommend`: what this machine's hardware can run, and which models
//! from the catalog to pull for it.
//!
//! It reads the machine (the memory a model may use on its GPU, or all of
//! memory where there is none, and which engines are installed), judges the
//! catalog against it the way `hedos pull` and the shelf's pull screen do,
//! and prints the picks. Nothing is pulled; `hedos pull <name>` does that.

use clap::Args;
use kernel::install::catalog::InstallCategory;
use kernel::install::installed::installed_names;
use kernel::install::recommend::{Ask, Note, Recommendation, Status, recommend};
use kernel::machine::{DeviceKind, Engine, Machine, Placement};
use kernel::profiles::{FitVerdict, Scale};
use kernel::records::byte_format::format_bytes;
use serde_json::{Value, json};

use crate::error::CliError;
use crate::support::machine;
use crate::support::output::Out;
use crate::support::recommend::notes_label;
use crate::support::session::Session;
use crate::support::shelf_table::verdict_label;
use crate::support::table;
use crate::support::text;

/// Arguments for `recommend`.
#[derive(Args)]
pub struct RecommendArgs {
    /// Only this kind of model: chat, code, voice or image. Repeat for more.
    #[arg(long = "kind", value_name = "KIND", value_parser = parse_kind)]
    kinds: Vec<InstallCategory>,
    /// Every model in the catalog, each with why it is or is not a pick.
    #[arg(long)]
    all: bool,
}

fn parse_kind(name: &str) -> Result<InstallCategory, String> {
    InstallCategory::parse(name).ok_or_else(|| {
        let kinds: Vec<&str> = InstallCategory::ALL
            .iter()
            .map(InstallCategory::as_str)
            .collect();
        format!("{name} is not a kind. use one of {}", kinds.join(", "))
    })
}

/// Run the `recommend` command.
pub async fn run(args: RecommendArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf().await;
    let installed = installed_names(&shelf);
    let machine = machine::machine();
    let ask = Ask {
        categories: &args.kinds,
        installed: &installed,
        all: true,
    };
    let judged = recommend(&machine, &ask);
    if out.is_json() {
        out.json(&document(&machine, &judged, args.all));
    } else {
        out.line(&report(&machine, &judged, args.all).join("\n"));
    }
    Ok(())
}

/// Whether `rec` is shown: always with `--all`, else when it is
/// [listed](Status::is_listed).
fn shown(rec: &Recommendation, all: bool) -> bool {
    all || rec.status.is_listed()
}

/// The report: the machine, what to install first, each kind's models, and
/// how to go on.
fn report(machine: &Machine, judged: &[Recommendation], all: bool) -> Vec<String> {
    let mut lines = machine_lines(machine);
    let shown: Vec<&Recommendation> = judged.iter().filter(|rec| shown(rec, all)).collect();
    lines.extend(install_lines(&shown));

    let noted = shown.iter().any(|rec| !rec.notes.is_empty());
    let rows: Vec<Vec<String>> = shown.iter().map(|rec| row(rec, noted)).collect();
    let widths = table::widths(&rows, None);
    for category in InstallCategory::ALL {
        let in_category: Vec<&Recommendation> = judged
            .iter()
            .filter(|rec| rec.entry.category == category)
            .collect();
        if in_category.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(category.as_str().to_owned());
        let mut any = false;
        for (rec, cells) in shown.iter().zip(&rows) {
            if rec.entry.category == category {
                lines.push(format!("  {}", table::row(cells, &widths)));
                any = true;
            }
        }
        if !any {
            lines.push(format!("  {}", nothing_line(&in_category)));
        }
    }
    lines.push(String::new());
    lines.push(if all {
        "hedos pull <name> fetches one".to_owned()
    } else {
        "hedos pull <name> fetches one · hedos recommend --all shows every model and why".to_owned()
    });
    lines
}

/// The machine in two or three lines: the chip, cores and memory; what a
/// model may use and where it runs; the free disk.
fn machine_lines(machine: &Machine) -> Vec<String> {
    let cores = match (machine.cores.performance, machine.cores.efficiency) {
        (Some(performance), Some(efficiency)) => {
            format!("{performance} performance + {efficiency} efficiency cores")
        }
        _ => text::count(machine.cores.logical as usize, "core"),
    };
    let mut first = Vec::new();
    first.extend(machine.chip.clone());
    if machine.cores.logical > 0 {
        first.push(cores);
    }
    first.push(format!("{} GiB memory", gib(machine.memory_bytes)));
    let mut lines = vec![first.join(" · ")];
    lines.push(placement_line(machine));
    lines.extend(disk_line(machine));
    lines
}

/// What a model may use here, and where it runs, as Ollama and llama.cpp
/// would place it.
fn placement_line(machine: &Machine) -> String {
    let Some(budget) = machine
        .budget(Engine::Ollama)
        .filter(|budget| budget.placement != Placement::Cpu)
    else {
        return "no GPU found: models run on the processor, from memory".to_owned();
    };
    let figure = gib(budget.bytes);
    if budget.scale == Scale::System {
        return format!("{figure} GiB for models, shared with the GPU");
    }
    if machine
        .devices
        .iter()
        .any(|device| device.kind == DeviceKind::Unified)
    {
        return format!("{figure} GiB for models on the GPU, its Metal working set");
    }
    let cards: Vec<&str> = machine
        .devices
        .iter()
        .filter(|device| device.kind == DeviceKind::Discrete && device.usable())
        .map(|device| device.name.as_str())
        .collect();
    format!(
        "{figure} GiB for models on {} ({}); a larger model runs partly from memory",
        cards.join(" + "),
        machine.devices_from.as_str()
    )
}

/// The free disk where pulls land: one figure when every provider's pulls
/// land on the same space, else one per provider.
fn disk_line(machine: &Machine) -> Option<String> {
    let first = machine.free_disk.first()?;
    if machine
        .free_disk
        .iter()
        .all(|free| free.bytes == first.bytes)
    {
        return Some(format!("{} GiB free on disk", gib(first.bytes)));
    }
    let parts: Vec<String> = machine
        .free_disk
        .iter()
        .map(|free| {
            format!(
                "{} GiB free for {}",
                gib(free.bytes),
                free.provider.as_str()
            )
        })
        .collect();
    Some(parts.join(" · "))
}

/// The install hints the shown models carry, each said once.
fn install_lines(shown: &[&Recommendation]) -> Vec<String> {
    let mut hints: Vec<&'static str> = Vec::new();
    for note in shown.iter().flat_map(|rec| &rec.notes) {
        if let Note::Install { hint, .. } = note
            && !hints.contains(hint)
        {
            hints.push(hint);
        }
    }
    hints.into_iter().map(str::to_owned).collect()
}

/// A model's row: its name, download size, where it stands, what to know
/// first when any row has something (`noted`), and what it is for.
fn row(rec: &Recommendation, noted: bool) -> Vec<String> {
    let mut cells = vec![
        rec.entry.reference.clone(),
        format_bytes(rec.entry.download_size()),
        standing(rec),
    ];
    if noted {
        cells.push(notes_label(&rec.notes));
    }
    cells.push(rec.entry.blurb.clone());
    cells
}

/// Where a model stands, in a word or two: `fits`, `tight`, `spills` for a
/// pick; `also fits` for one the larger picks passed over; `on shelf`, `too
/// big`, or `can't run here`.
fn standing(rec: &Recommendation) -> String {
    let fit = rec.fit;
    let word = || match fit {
        Some(fit) if fit.placement == Placement::Spill => "spills",
        Some(fit) => verdict_label(Some(fit.assessment.verdict)),
        None => "",
    };
    match rec.status {
        Status::Pick => word().to_owned(),
        Status::Fits => format!("also {}", word()),
        Status::OnShelf => "on shelf".to_owned(),
        Status::TooLarge => verdict_label(Some(FitVerdict::TooLarge)).to_owned(),
        Status::NoEngine => "can't run here".to_owned(),
    }
}

/// Why a kind has nothing to show without `--all`.
fn nothing_line(in_category: &[&Recommendation]) -> &'static str {
    if in_category.iter().all(|rec| rec.status == Status::NoEngine) {
        "nothing of this kind runs on this machine"
    } else {
        "nothing of this kind fits this machine"
    }
}

fn gib(bytes: u64) -> String {
    text::gib_short(i64::try_from(bytes).unwrap_or(i64::MAX))
}

/// The machine and the recommendations as one JSON document.
fn document(machine: &Machine, judged: &[Recommendation], all: bool) -> Value {
    let recommendations: Vec<Value> = judged
        .iter()
        .filter(|rec| shown(rec, all))
        .map(recommendation_json)
        .collect();
    json!({
        "machine": machine_json(machine),
        "recommendations": recommendations,
    })
}

fn machine_json(machine: &Machine) -> Value {
    let devices: Vec<Value> = machine
        .devices
        .iter()
        .map(|device| {
            json!({
                "name": device.name,
                "kind": device.kind.as_str(),
                "memory_bytes": device.memory_bytes,
            })
        })
        .collect();
    let free_disk: Vec<Value> = machine
        .free_disk
        .iter()
        .map(|free| json!({ "provider": free.provider.as_str(), "bytes": free.bytes }))
        .collect();
    json!({
        "os": machine.os.as_str(),
        "arch": machine.arch,
        "chip": machine.chip,
        "cores": {
            "performance": machine.cores.performance,
            "efficiency": machine.cores.efficiency,
            "logical": machine.cores.logical,
        },
        "memory_bytes": machine.memory_bytes,
        "devices": devices,
        "devices_from": machine.devices_from.as_str(),
        "models_budget_bytes": machine.models_budget_bytes(),
        "free_disk": free_disk,
        "engines": {
            "ollama": machine.engines.ollama,
            "llama_cpp": machine.engines.llama_cpp,
            "uv": machine.engines.uv,
        },
    })
}

fn recommendation_json(rec: &Recommendation) -> Value {
    let entry = &rec.entry;
    let notes: Vec<Value> = rec
        .notes
        .iter()
        .map(|note| match note {
            Note::Install { engine, hint } => json!({
                "kind": "install",
                "engine": engine.as_str(),
                "hint": hint,
            }),
            Note::Disk { needs, free } => json!({
                "kind": "disk",
                "needs_bytes": needs,
                "free_bytes": free,
            }),
        })
        .collect();
    json!({
        "kind": entry.category.as_str(),
        "reference": entry.reference,
        "provider": entry.provider.as_str(),
        "name": entry.name,
        "blurb": entry.blurb,
        "download_bytes": entry.download_bytes,
        "serving_bytes": entry.serving_bytes,
        "engine": entry.engine.as_str(),
        "status": rec.status.as_str(),
        "verdict": rec.fit.map(|fit| fit.assessment.verdict.as_str()),
        "required_bytes": rec.fit.map(|fit| fit.assessment.required_bytes),
        "placement": rec.fit.map(|fit| fit.placement.as_str()),
        "notes": notes,
    })
}

#[cfg(test)]
mod tests;
