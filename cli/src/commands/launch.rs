//! `hedos launch <harness>` — run a coding harness against a gateway served for
//! the life of that harness.
//!
//! The gateway binds an ephemeral port in this process, so there is nothing to
//! start first and no port to collide with. When the harness exits, the gateway
//! stops and its exit code becomes ours.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use clap::Args;
use gateway::server;
use kernel::records::{Capability, JsonValue, ModelRecord};
use tokio::sync::oneshot;

use crate::error::CliError;
use crate::support::harnesses::{self, HarnessSpec};
use crate::support::interactive;
use crate::support::output::Out;
use crate::support::payload;
use crate::support::serving;
use crate::support::session::Session;
use crate::support::spinner::Spinner;

/// Arguments for `launch`.
#[derive(Args)]
pub struct LaunchArgs {
    /// The harness to launch. Omit to pick one interactively.
    harness: Option<String>,
    /// The model to serve it (name, alias, or id). Omit to pick one.
    #[arg(short, long)]
    model: Option<String>,
    /// Arguments passed through to the harness, after `--`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    passthrough: Vec<String>,
}

/// Run the `launch` command; blocks until the harness exits.
pub async fn run(args: LaunchArgs, out: &Out) -> Result<(), CliError> {
    let spec = choose_harness(out, args.harness.as_deref())?;
    let program = locate(spec)?;

    let session = Session::open()?;
    let shelf = session.shelf_or_discover().await?;
    let warm = session.warm_set();

    // Tool calling is what every harness but aider drives the model through, so
    // the picker offers only tool-capable models for those. An explicit `-m`
    // still resolves against the whole shelf, so a named model that can't do
    // tools gets the precise pre-flight reason rather than "no such model".
    let needed = if spec.needs_tools {
        Capability::tools()
    } else {
        Capability::chat()
    };
    let pick_capability = if args.model.is_some() {
        Capability::chat()
    } else {
        needed.clone()
    };
    let record = interactive::choose_model(
        out,
        args.model.as_deref(),
        &shelf,
        Some(&pick_capability),
        &format!("model for {}", spec.display),
        &warm,
    )?;
    // The ids the harness sends back must be the ones `/v1/models` advertises,
    // or the gateway cannot resolve them.
    let wire_id = |record: &ModelRecord| record.wire_id().to_owned();
    let model = wire_id(record);
    // The models offered inside the harness's own picker: the same tool-capable
    // set, so switching models there can't land on one that won't work either.
    let available: Vec<String> = shelf
        .iter()
        .filter(|record| record.capabilities.contains(&needed))
        .map(wire_id)
        .collect();

    preflight(&session, record, spec, out).await?;

    let max_inference = session.settings.gateway.max_concurrent_inference.max(1) as usize;
    let audit_dir = session.dirs.sub("gateway");
    let config_dir = session.dirs.sub("launch");

    let listener = serving::bind(0).await?;
    let port = listener.local_addr()?.port();
    let plan = spec.plan(port, &model, &available, &config_dir);

    materialize(&plan, &config_dir)?;
    let (server_task, stop) = serve_for_launch(session.kernel, &audit_dir, max_inference, listener);

    out.line(&format!(
        "{} · {model} · gateway on 127.0.0.1:{port}",
        spec.display
    ));
    out.json(&serde_json::json!({
        "harness": spec.key,
        "model": model,
        "port": port,
        "dialect": spec.dialect.as_str(),
    }));

    // Ctrl-C belongs to the harness: it is the foreground TUI, and the same SIGINT
    // reaches it directly. Holding a handler here keeps the default disposition
    // from killing this process — and the gateway with it — out from under a
    // child that meant to handle the interrupt itself.
    tokio::spawn(async {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    });

    let status = spawn_harness(&program, spec, &plan, &args.passthrough).await?;
    let _ = stop.send(());
    if let Ok(Err(error)) = server_task.await {
        out.err(&format!("gateway stopped with an error: {error}"));
    }

    exit_result(status, spec)
}

/// Write the plan's generated config files under `config_dir`.
fn materialize(plan: &harnesses::LaunchPlan, config_dir: &Path) -> Result<(), CliError> {
    if plan.files.is_empty() {
        return Ok(());
    }
    create_config_dir(config_dir)?;
    for (path, contents) in &plan.files {
        write_config(path, contents)?;
    }
    Ok(())
}

/// Create `config_dir`, owner-only on Unix — it carries the gateway's URL and
/// placeholder token, unreadable to other local accounts on a shared host. A
/// pre-existing dir with looser permissions is tightened best-effort; only the
/// creation itself is fatal.
fn create_config_dir(config_dir: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(config_dir)
            .map_err(|error| {
                CliError::new(format!(
                    "could not create {} — {error}",
                    config_dir.display()
                ))
            })?;
        let _ = std::fs::set_permissions(config_dir, std::fs::Permissions::from_mode(0o700));
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(config_dir).map_err(|error| {
            CliError::new(format!(
                "could not create {} — {error}",
                config_dir.display()
            ))
        })
    }
}

/// Serve the gateway on `listener` until the returned sender fires (or drops).
fn serve_for_launch(
    kernel: runtime::facade::Kernel,
    audit_dir: &Path,
    max_inference: usize,
    listener: tokio::net::TcpListener,
) -> (
    tokio::task::JoinHandle<std::io::Result<()>>,
    oneshot::Sender<()>,
) {
    let router = serving::router(kernel, audit_dir, max_inference);
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        server::serve_with_shutdown(listener, router, async {
            let _ = stopped.await;
        })
        .await
    });
    (task, stop)
}

/// The harness's exit status as ours.
fn exit_result(status: std::process::ExitStatus, spec: &HarnessSpec) -> Result<(), CliError> {
    match status.code() {
        Some(0) => Ok(()),
        Some(code) => Err(CliError::with_code(
            format!("{} exited with status {code}", spec.display),
            code,
        )),
        // A signal death has no exit code. Ctrl-C is the supported way to leave
        // the harness, so SIGINT stays a quiet success; any other signal
        // (SIGSEGV, SIGKILL) exits with the conventional 128+signal rather than
        // reading as success to whatever wraps `hedos launch`.
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                const SIGINT: i32 = 2;
                if let Some(signal) = status.signal()
                    && signal != SIGINT
                {
                    return Err(CliError::with_code(
                        format!("{} was killed by signal {signal}", spec.display),
                        128 + signal,
                    ));
                }
            }
            Ok(())
        }
    }
}

/// Check the model can actually serve the harness, before the harness starts.
///
/// Two gates. First the same capability check the gateway applies to every
/// request: a tool-needing harness requires the `tools` capability, refused
/// here with the precise reason instead of as a 400 on every request inside
/// the harness. The probe below can't catch this case on its own — a Python
/// sidecar serves a tools request even for a model whose template declares
/// none (falling back to a generic tool prompt), so a probe through one
/// "succeeds" against a model the gateway will refuse.
///
/// Then one throwaway request shaped like the ones the harness will send. This
/// catches the failures only the runtime can report: a backend that is down (a
/// stopped Ollama daemon, an out-of-memory GPU), and a tool refusal from a
/// model whose template we couldn't read up front — Ollama, for one, keeps
/// some chat templates inside the GGUF rather than on disk. It also leaves the
/// model loaded, so the first real request is warm.
async fn preflight(
    session: &Session,
    record: &ModelRecord,
    spec: &HarnessSpec,
    out: &Out,
) -> Result<(), CliError> {
    if spec.needs_tools && !record.capabilities.contains(&Capability::tools()) {
        return Err(CliError::new(no_tools_reason(record, spec)));
    }

    let mut spinner = Spinner::start(out);
    spinner.set(&format!("checking {}", record.display_name()));

    let mut payload = payload::chat(vec![payload::message("user", "hi")], Some(1));
    if spec.needs_tools {
        payload.insert("tools".to_owned(), JsonValue::Array(vec![probe_tool()]));
    }

    let result = async {
        let mut stream = session
            .kernel
            .invoke(&record.id, Capability::chat(), JsonValue::Object(payload))
            .await?;
        while let Some(chunk) = stream.recv().await {
            chunk?;
        }
        Ok::<(), CliError>(())
    }
    .await;
    spinner.clear();

    result.map_err(|error| {
        let hint = if spec.needs_tools && mentions_tools(&error.message) {
            format!(
                "\n  {} drives the model through tool calls, so it needs a model that supports them.\n  Try `hedos ls --capability tools` and pick another, or use aider, which does not need tools.",
                spec.display
            )
        } else {
            String::new()
        };
        CliError::new(format!(
            "{} can't serve {} — {}{hint}",
            record.display_name(),
            spec.display,
            error.message
        ))
    })
}

/// Why a model without the `tools` capability can't serve `spec`, phrased by
/// cause: a chat template that declares no tools, or a runtime that never
/// forwards them.
fn no_tools_reason(record: &ModelRecord, spec: &HarnessSpec) -> String {
    let why = if record.supports_tools == Some(false) {
        format!(
            "{}'s chat template does not declare tool calls",
            record.display_name()
        )
    } else {
        let runtime = record
            .runtime
            .id
            .as_ref()
            .map_or("its runtime", |id| id.as_str());
        format!(
            "{} is served by {runtime}, which does not forward tool calls to the model",
            record.display_name()
        )
    };
    format!(
        "{why}, and {} drives the model through them.\n  Try `hedos ls --capability tools` and pick another, or use aider, which does not need tools.",
        spec.display
    )
}

/// The smallest well-formed tool spec, used only to ask the runtime whether it
/// can accept tools at all.
fn probe_tool() -> JsonValue {
    let mut parameters = BTreeMap::new();
    parameters.insert("type".to_owned(), JsonValue::String("object".to_owned()));
    parameters.insert("properties".to_owned(), JsonValue::Object(BTreeMap::new()));

    let mut tool = BTreeMap::new();
    tool.insert("name".to_owned(), JsonValue::String("noop".to_owned()));
    tool.insert(
        "description".to_owned(),
        JsonValue::String("does nothing".to_owned()),
    );
    tool.insert("parameters".to_owned(), JsonValue::Object(parameters));
    JsonValue::Object(tool)
}

/// Whether a runtime error is about tool support, so the advice can name the
/// real remedy rather than a generic one.
fn mentions_tools(message: &str) -> bool {
    let lowered = message.to_lowercase();
    lowered.contains("tool") || lowered.contains("function calling")
}

/// Spawn the harness with the plan's environment layered over our own, sharing
/// this terminal. A harness is a full-screen TUI, so stdio is inherited rather
/// than captured, and nothing bounds how long it runs.
async fn spawn_harness(
    program: &Path,
    spec: &HarnessSpec,
    plan: &harnesses::LaunchPlan,
    passthrough: &[String],
) -> Result<std::process::ExitStatus, CliError> {
    let mut command = tokio::process::Command::new(program);
    command.args(passthrough);
    command.args(&plan.args);
    for (key, value) in &plan.env {
        command.env(key, value);
    }
    command.stdin(Stdio::inherit());
    command.stdout(Stdio::inherit());
    command.stderr(Stdio::inherit());

    let mut child = command
        .spawn()
        .map_err(|error| CliError::new(format!("could not start {}: {error}", spec.binary)))?;
    child
        .wait()
        .await
        .map_err(|error| CliError::new(format!("{} failed: {error}", spec.binary)))
}

/// Resolve `arg` to a harness, or let the user pick one when it is absent.
fn choose_harness(out: &Out, arg: Option<&str>) -> Result<&'static HarnessSpec, CliError> {
    if let Some(key) = arg {
        return harnesses::find(key).ok_or_else(|| {
            let known = harnesses::HARNESSES
                .iter()
                .map(|harness| harness.key)
                .collect::<Vec<_>>()
                .join(", ");
            CliError::new(format!(
                "unknown harness \"{key}\" — known harnesses: {known}"
            ))
        });
    }
    if !interactive::is_interactive(out) {
        return Err(CliError::new(
            "no harness given — pass one, or run in a terminal to pick one",
        ));
    }

    // Only offer what is actually installed, so the picker can't lead to a
    // "not found" a moment later.
    let installed: Vec<&HarnessSpec> = harnesses::HARNESSES
        .iter()
        .filter(|harness| kernel::fs::find_on_path(harness.binary).is_some())
        .collect();
    if installed.is_empty() {
        let listing = harnesses::HARNESSES
            .iter()
            .map(|harness| format!("{} · {}", harness.key, harness.homepage))
            .collect::<Vec<_>>()
            .join("\n  ");
        return Err(CliError::new(format!(
            "no supported harness found on PATH — install one of:\n  {listing}"
        )));
    }

    let labels: Vec<String> = installed
        .iter()
        .map(|harness| harness.display.to_owned())
        .collect();
    let index = interactive::select_index("harness", &labels)?;
    installed
        .get(index)
        .copied()
        .ok_or_else(|| CliError::new("nothing selected"))
}

/// The harness's executable, or a user-facing error pointing at its homepage.
fn locate(spec: &HarnessSpec) -> Result<PathBuf, CliError> {
    kernel::fs::find_on_path(spec.binary).ok_or_else(|| {
        CliError::new(format!(
            "{} is not installed — `{}` is not on your PATH. See {}",
            spec.display, spec.binary, spec.homepage
        ))
    })
}

/// Write a generated harness config, replacing any previous one.
///
/// These live under the hedos data dir, never beside the user's own config, so
/// launching a harness can't disturb how it behaves when run directly. They
/// carry the gateway's URL and placeholder token, so on Unix the file is
/// created owner-only.
fn write_config(path: &Path, contents: &str) -> Result<(), CliError> {
    write_config_contents(path, contents)
        .map_err(|error| CliError::new(format!("could not write {} — {error}", path.display())))
}

/// The fallible half of [`write_config`], kept separate so its `io::Result`
/// composes cleanly across the Unix/non-Unix branches.
fn write_config_contents(path: &Path, contents: &str) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A unique temporary directory removed on drop. Built without the
    /// `tempfile` crate to honor the project's minimal-dependency policy.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let pid = std::process::id();
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0);
            let path =
                std::env::temp_dir().join(format!("hedos-launch-test-{pid}-{nanos}-{unique}"));
            std::fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    #[cfg(unix)]
    fn write_config_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        let path = dir.path.join("config.json");
        write_config(&path, "{}").expect("write config");

        let mode = std::fs::metadata(&path)
            .expect("config file exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    #[cfg(unix)]
    fn config_dir_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new();
        let config_dir = dir.path.join("launch");
        create_config_dir(&config_dir).expect("create config dir");

        let mode = std::fs::metadata(&config_dir)
            .expect("config dir exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o700);
    }
}

#[cfg(all(test, unix))]
mod exit_result_tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn spec() -> &'static HarnessSpec {
        harnesses::find("aider").expect("aider is registered")
    }

    #[test]
    fn normal_exit_zero_is_ok() {
        let status = std::process::ExitStatus::from_raw(0);
        assert!(exit_result(status, spec()).is_ok());
    }

    #[test]
    fn normal_exit_nonzero_is_an_error_with_the_matching_code() {
        let status = std::process::ExitStatus::from_raw(3 << 8);
        let error = exit_result(status, spec()).expect_err("nonzero exit should error");
        assert_eq!(error.code, 3);
        assert!(error.message.contains("status 3"));
    }

    #[test]
    fn sigint_is_a_quiet_success() {
        let status = std::process::ExitStatus::from_raw(2);
        assert!(exit_result(status, spec()).is_ok());
    }

    #[test]
    fn any_other_signal_exits_with_128_plus_the_signal() {
        let status = std::process::ExitStatus::from_raw(11);
        let error = exit_result(status, spec()).expect_err("sigsegv should error");
        assert_eq!(error.code, 139);
        assert!(error.message.contains("signal 11"));
    }
}
