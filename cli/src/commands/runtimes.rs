//! `hedos runtimes`: the manifest runtimes on this machine, and the consent
//! that lets one run.
//!
//! A manifest runtime runs code on the host, so it neither bids on a model nor
//! serves one until it is approved here. The approval is bound to a hash of the
//! runtime's files, and editing any of them asks for it again. A runtime whose
//! command is a release binary pinned in its manifest has that binary
//! downloaded and checked here, once the approval is given.

use clap::{Args, Subcommand};
use kernel::manifests::RuntimeManifest;
use kernel::records::ModelRecord;
use runtime::boot::HedosDirs;
use runtime::manifests::{
    COMMANDS_DIR, HostConsent, host_build, host_consent, install, installed, servable_models,
};
use runtime::settings::{ModelsSettings, SettingsStore};

use crate::error::CliError;
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::session::Session;
use crate::support::table;

/// Arguments for `runtimes`.
#[derive(Args)]
pub struct RuntimesArgs {
    #[command(subcommand)]
    command: Option<RuntimesCommand>,
}

#[derive(Subcommand)]
enum RuntimesCommand {
    /// List every manifest runtime and where its approval stands.
    Ls,
    /// Let a runtime run code on this machine.
    Approve(ApproveArgs),
    /// Take a runtime's approval back.
    Revoke(RuntimeArgs),
}

#[derive(Args)]
struct ApproveArgs {
    /// The runtime's id, as `hedos runtimes` lists it.
    id: String,
    /// Approve without being asked to confirm.
    #[arg(short, long)]
    yes: bool,
}

#[derive(Args)]
struct RuntimeArgs {
    /// The runtime's id, as `hedos runtimes` lists it.
    id: String,
}

/// Run the `runtimes` command.
pub async fn run(args: RuntimesArgs, out: &Out) -> Result<(), CliError> {
    match args.command.unwrap_or(RuntimesCommand::Ls) {
        RuntimesCommand::Ls => list(out).await,
        RuntimesCommand::Approve(args) => approve(args, out).await,
        RuntimesCommand::Revoke(args) => revoke(args, out).await,
    }
}

async fn list(out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let runtimes = session.kernel.manifest_runtimes();
    for issue in session.kernel.runtime_issues() {
        out.err(&format!("issue: {issue}"));
    }

    let entries: Vec<serde_json::Value> = runtimes
        .iter()
        .map(|manifest| {
            serde_json::json!({
                "id": manifest.id,
                "capabilities": manifest.capabilities,
                "consent": standing(manifest, &session.settings.models).1,
                "downloaded": manifest
                    .install
                    .as_ref()
                    .map(|_| installed(manifest, &commands(&session.dirs)).is_some()),
                "network": manifest.permissions.network,
                "directory": manifest.directory,
                "models": model_names(manifest, &shelf),
            })
        })
        .collect();
    out.json(&serde_json::json!({
        "runtimes": entries,
        "issues": session.kernel.runtime_issues(),
    }));

    if runtimes.is_empty() {
        out.line(&format!(
            "No manifest runtimes. Put one in {}.",
            runtime::boot::runtimes_directory(&session.dirs).display()
        ));
        return Ok(());
    }
    let rows: Vec<Vec<String>> = runtimes
        .iter()
        .map(|manifest| {
            let capabilities: Vec<&str> = manifest
                .capabilities
                .iter()
                .map(|capability| capability.as_str())
                .collect();
            vec![
                manifest.id.clone(),
                capabilities.join(","),
                described(manifest, &session.settings.models, &session.dirs),
                or_dash(model_names(manifest, &shelf).join(", ")),
            ]
        })
        .collect();
    out.line(&table::render(
        &["RUNTIME", "SERVES", "CONSENT", "MODELS"],
        &rows,
    ));
    Ok(())
}

async fn approve(args: ApproveArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let manifest = find(&session, &args.id)?;
    if manifest.vm.is_some() {
        return Err(CliError::new(format!(
            "{} runs in a VM, which this build cannot start; there is nothing to approve",
            manifest.id
        )));
    }
    let Some(hash) = manifest.content_hash.as_deref() else {
        return Err(CliError::new(format!(
            "{} has no content hash to bind an approval to",
            manifest.id
        )));
    };
    if manifest.install.is_some() {
        host_build(manifest).map_err(|error| CliError::new(error.to_string()))?;
    }
    if host_consent(manifest, &session.settings.models).is_approved() {
        // An approval whose download failed or was removed is finished here.
        if manifest.install.is_some() && installed(manifest, &commands(&session.dirs)).is_none() {
            fetch(manifest, &session.dirs, out).await?;
        }
        out.line(&format!("{} is already approved", manifest.id));
        out.json(&serde_json::json!({
            "runtime": manifest.id,
            "consent": "approved",
            "models": model_names(manifest, &shelf),
        }));
        return Ok(());
    }

    for line in consent_card(manifest, hash, &shelf) {
        out.err(&line);
    }
    if !args.yes {
        if !interactive::is_interactive(out) {
            return Err(CliError::new(
                "approving a runtime needs a yes: run this in a terminal, or pass --yes",
            ));
        }
        if !interactive::confirm(&format!("Let {} run on this machine?", manifest.id), false)? {
            return Err(CliError::new("not approved"));
        }
    }

    // The binary is fetched before the approval is written, so a download that
    // fails leaves nothing half approved.
    if manifest.install.is_some() {
        fetch(manifest, &session.dirs, out).await?;
    }
    SettingsStore::discover().approve_runtime(
        &manifest.id,
        Some(hash),
        manifest.permissions.network,
    )?;
    let id = manifest.id.clone();
    out.line(&format!("{id} approved"));
    let resolved = settle(&id, out).await?;
    out.json(&serde_json::json!({ "runtime": id, "consent": "approved", "models": resolved }));
    Ok(())
}

async fn revoke(args: RuntimeArgs, out: &Out) -> Result<(), CliError> {
    let session = Session::open()?;
    // An approval can outlive its manifest, so revoking goes by what the
    // settings hold, not by what loaded.
    let known = session
        .settings
        .models
        .approved_host_runtimes
        .contains(&args.id);
    if !known {
        return Err(CliError::new(format!("{} holds no approval", args.id)));
    }
    SettingsStore::discover().revoke_runtime(&args.id)?;
    out.line(&format!("{} revoked", args.id));
    settle(&args.id, out).await?;
    out.json(&serde_json::json!({ "runtime": args.id, "consent": "unapproved" }));
    Ok(())
}

/// Rescan and re-resolve the shelf under the settings as they now stand, and say
/// which models `id` serves as a result. A kernel reads its approvals once, at
/// boot, so this opens a fresh one, and a gateway already running needs a
/// restart. It is a scan and not only a resolve because a manifest overrides
/// what a scanner hinted about a model, and only a scan puts those hints back
/// once the manifest lets go.
async fn settle(id: &str, out: &Out) -> Result<Vec<String>, CliError> {
    let session = Session::open()?;
    session.discover().await?;
    let shelf = session.shelf().await;
    let served: Vec<String> = shelf
        .iter()
        .filter(|record| {
            record
                .runtime
                .id
                .as_ref()
                .is_some_and(|runtime| runtime.as_str() == id)
        })
        .map(|record| record.display_name().to_owned())
        .collect();
    if !served.is_empty() {
        out.line(&format!("now serving {}", served.join(", ")));
    }
    if session.live_gateway().await.is_some() {
        out.err(
            "a gateway is running with the old approvals; restart `hedos serve` to pick this up",
        );
    }
    Ok(served)
}

/// Where downloaded commands are kept.
fn commands(dirs: &HedosDirs) -> std::path::PathBuf {
    dirs.sub(COMMANDS_DIR)
}

/// Download, check and unpack `manifest`'s pinned binary for this machine,
/// saying so while it does.
async fn fetch(manifest: &RuntimeManifest, dirs: &HedosDirs, out: &Out) -> Result<(), CliError> {
    if let (Some(pinned), Ok(build)) = (&manifest.install, host_build(manifest)) {
        out.err(&format!(
            "downloading {} {} ({})",
            manifest.id, pinned.version, build.url
        ));
    }
    let path = install(manifest, &commands(dirs))
        .await
        .map_err(|error| CliError::new(error.to_string()))?;
    out.err(&format!("checked and installed {}", path.display()));
    Ok(())
}

fn find<'a>(session: &'a Session, id: &str) -> Result<&'a RuntimeManifest, CliError> {
    session
        .kernel
        .manifest_runtimes()
        .iter()
        .find(|manifest| manifest.id == id)
        .ok_or_else(|| {
            CliError::new(format!(
                "no manifest runtime named {id}; see `hedos runtimes`"
            ))
        })
}

/// What the user is agreeing to, in the terminal's stead of a consent card.
fn consent_card(manifest: &RuntimeManifest, hash: &str, shelf: &[ModelRecord]) -> Vec<String> {
    let mut lines = vec![format!("{} runs code on this machine.", manifest.id)];
    if let Some(directory) = &manifest.directory {
        lines.push(format!("  files    {}", directory.display()));
    }
    if let Some(serve) = &manifest.serve {
        lines.push(format!("  runs     {}", serve.entrypoint));
    }
    if let Some(invoke) = &manifest.invoke {
        lines.push(format!("  runs     {}", invoke.command));
    }
    if let Some(env) = &manifest.env {
        lines.push(format!("  installs {}", env.lockfile));
    }
    if let (Some(pinned), Ok(build)) = (&manifest.install, host_build(manifest)) {
        lines.push(format!("  fetches  {} {}", pinned.version, build.url));
        lines.push(format!("  sha256   {}", build.sha256));
    }
    lines.push(format!(
        "  declares paths {}, network {}",
        manifest.permissions.paths.join(", "),
        if manifest.permissions.network {
            "yes"
        } else {
            "no"
        }
    ));
    lines.push(format!(
        "  models   {}",
        or_dash(model_names(manifest, shelf).join(", "))
    ));
    lines.push(format!("  hash     {hash}"));
    lines.push(
        "What it declares is its own account and is not enforced: it runs as you, unsandboxed, \
         with your files and your network in reach."
            .to_owned(),
    );
    lines.push("Editing any of its files asks for this approval again.".to_owned());
    lines
}

fn model_names(manifest: &RuntimeManifest, shelf: &[ModelRecord]) -> Vec<String> {
    servable_models(manifest, shelf)
        .into_iter()
        .map(|record| record.display_name().to_owned())
        .collect()
}

fn or_dash(text: String) -> String {
    if text.is_empty() {
        "-".to_owned()
    } else {
        text
    }
}

/// Where a runtime stands, in the table's words: an approved runtime whose
/// pinned binary is not in place yet says so.
fn described(manifest: &RuntimeManifest, models: &ModelsSettings, dirs: &HedosDirs) -> String {
    let (words, slug) = standing(manifest, models);
    let missing = manifest.install.is_some() && installed(manifest, &commands(dirs)).is_none();
    if slug == "approved" && missing {
        return "approved, not downloaded".to_owned();
    }
    words.to_owned()
}

/// Where a runtime stands, as the table's words and the JSON's slug. A `[vm]`
/// runtime is not waiting on an approval: no approval would make it run here.
fn standing(manifest: &RuntimeManifest, models: &ModelsSettings) -> (&'static str, &'static str) {
    if manifest.vm.is_some() {
        return ("needs a VM", "unsupported");
    }
    match host_consent(manifest, models) {
        HostConsent::Approved => ("approved", "approved"),
        HostConsent::Changed => ("changed since approval", "changed"),
        HostConsent::Unapproved => ("needs approval", "unapproved"),
    }
}
