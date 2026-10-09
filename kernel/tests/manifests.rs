//! Tests for runtime-manifest parsing/validation and install provenance.

mod support;

use kernel::manifests::{RuntimeManifest, RuntimeProvenance};
use kernel::records::{Capability, ExecutionMode, Modality};
use support::TempDir;

fn parse(text: &str) -> Result<RuntimeManifest, kernel::manifests::ManifestValidationError> {
    RuntimeManifest::parse(text, None)
}

#[test]
fn loads_a_valid_invoke_manifest() {
    let text = r#"
        id           = "kokoro-cli"
        modalities   = ["speech"]
        capabilities = ["speak"]
        execution    = "sync"
        detect       = { extension = "pth" }

        [invoke]
        command = "kokoro-tool --model {model} --text {prompt}"

        [permissions]
        network = false
        paths   = ["{model}", "{workdir}"]
    "#;
    let manifest = parse(text).expect("valid");
    assert_eq!(manifest.id, "kokoro-cli");
    assert_eq!(manifest.modalities, vec![Modality::from("speech")]);
    assert_eq!(manifest.capabilities, vec![Capability::speak()]);
    assert_eq!(manifest.execution, ExecutionMode::Sync);
    assert_eq!(
        manifest
            .detect
            .as_ref()
            .and_then(|d| d.file_extension.as_deref()),
        Some("pth")
    );
    assert!(
        manifest
            .invoke
            .as_ref()
            .unwrap()
            .command
            .contains("{prompt}")
    );
    assert!(!manifest.permissions.network);
    assert_eq!(manifest.permissions.paths, vec!["{model}", "{workdir}"]);
}

#[test]
fn serve_manifest_defaults_protocol_and_env() {
    let text = r#"
        id           = "python:mflux-user"
        modalities   = ["image"]
        capabilities = ["image"]
        execution    = "job"

        [env]
        lockfile = "mflux.lock"

        [serve]
        entrypoint = "main.py"
    "#;
    let manifest = parse(text).expect("valid");
    let env = manifest.env.expect("env");
    assert_eq!(env.manager, "uv");
    assert_eq!(env.python, "3.12");
    assert_eq!(env.lockfile, "mflux.lock");
    let serve = manifest.serve.expect("serve");
    assert_eq!(serve.entrypoint, "main.py");
    assert_eq!(serve.wire_protocol, "ndjson+frames");
}

#[test]
fn a_serve_manifest_without_an_entrypoint_is_rejected() {
    let text = r#"
        id = "a"
        capabilities = ["image"]
        execution = "job"
        [serve]
        protocol = "ndjson+frames"
    "#;
    let err = parse(text).expect_err("rejected");
    assert!(err.to_string().contains("entrypoint"), "{err}");
}

#[test]
fn rejection_matrix() {
    let cases = [
        (
            "missing id",
            "capabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"x\"",
        ),
        (
            "no capabilities",
            "id = \"a\"\ncapabilities = []\nexecution = \"sync\"\n[invoke]\ncommand = \"x\"",
        ),
        (
            "bad execution",
            "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"warp\"\n[invoke]\ncommand = \"x\"",
        ),
        (
            "neither serve nor invoke",
            "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"",
        ),
        (
            "both serve and invoke",
            "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[serve]\nentrypoint = \"m.py\"\n[invoke]\ncommand = \"x\"",
        ),
        (
            "chat as job",
            "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"job\"\n[invoke]\ncommand = \"x\"",
        ),
        (
            "image as stream",
            "id = \"a\"\ncapabilities = [\"image\"]\nexecution = \"stream\"\n[invoke]\ncommand = \"x\"",
        ),
        (
            "invoke + stream",
            "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\n[invoke]\ncommand = \"x\"",
        ),
        (
            "empty invoke command",
            "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"\"",
        ),
    ];
    for (name, text) in cases {
        assert!(parse(text).is_err(), "expected rejection: {name}");
    }
}

#[test]
fn accepts_sync_invoke_job_invoke_and_serve_stream() {
    let good = [
        "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"x\"",
        "id = \"a\"\ncapabilities = [\"image\"]\nexecution = \"job\"\n[invoke]\ncommand = \"x\"",
        "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\n[serve]\nentrypoint = \"m.py\"",
    ];
    for text in good {
        assert!(parse(text).is_ok(), "expected acceptance: {text}");
    }
}

#[test]
fn invoke_stream_error_names_the_reason() {
    let text =
        "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\n[invoke]\ncommand = \"x\"";
    let err = parse(text).expect_err("rejected");
    assert!(
        err.to_string()
            .contains("invoke manifests run to completion"),
        "{err}"
    );
}

#[test]
fn manifest_id_slug_validation() {
    let manifest = |id: &str| {
        format!(
            "id = \"{id}\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"x\""
        )
    };
    for bad in ["../evil", "a/b", "..", "…", "a b"] {
        assert!(parse(&manifest(bad)).is_err(), "expected rejection: {bad}");
    }
    for good in ["python:kokoro-vm", "my-runtime_2.0", "a.b.c"] {
        assert!(
            parse(&manifest(good)).is_ok(),
            "expected acceptance: {good}"
        );
    }
}

#[test]
fn vm_manifest_rules() {
    let base = r#"
        id = "python:kokoro-vm"
        capabilities = ["speak"]
        execution = "sync"
        detect = { extension = "pth" }
        [invoke]
        command = "kokoro {prompt}"
        [vm]
        image = "ghcr.io/acme/kokoro@sha256:abc123"
    "#;
    assert!(
        parse(base).is_ok(),
        "a digest-pinned offline vm manifest is valid"
    );

    // A tag-only (non-digest) image is rejected.
    let tagged = base.replace("@sha256:abc123", ":latest");
    let err = parse(&tagged).expect_err("rejected");
    assert!(err.to_string().contains("digest-pinned"), "{err}");

    // network permission is rejected for vm runtimes.
    let networked = format!("{base}\n[permissions]\nnetwork = true");
    let err = parse(&networked).expect_err("rejected");
    assert!(
        err.to_string().contains("vm runtimes always run offline"),
        "{err}"
    );

    let offline = format!("{base}\n[permissions]\nnetwork = false");
    assert!(
        parse(&offline).is_ok(),
        "an explicit offline vm manifest is valid"
    );
}

#[test]
fn detect_rule_needs_a_file_or_extension() {
    let text = "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"x\"\n[detect]\ncontains = \"Flux\"";
    let err = parse(text).expect_err("rejected");
    assert!(err.to_string().contains("detect rule"), "{err}");
}

#[test]
fn provenance_round_trips_and_reads_community() {
    let dir = TempDir::new();
    let provenance = RuntimeProvenance::community();
    assert!(provenance.is_community());
    provenance.write(dir.path()).expect("write");

    let read = RuntimeProvenance::read(dir.path()).expect("read");
    assert_eq!(read.origin, provenance.origin);
    assert_eq!(read.installed_at, provenance.installed_at);
    assert!(read.is_community());

    let empty = TempDir::new();
    assert_eq!(
        RuntimeProvenance::read(empty.path()),
        None,
        "absent provenance is None"
    );
    assert!(!RuntimeProvenance::new("first-party").is_community());
}

#[test]
fn a_corrupt_provenance_reads_none_and_is_left_in_place() {
    let dir = TempDir::new();
    let file = dir.join(".provenance.json");
    std::fs::write(&file, b"{ not json").expect("write corrupt");

    assert_eq!(RuntimeProvenance::read(dir.path()), None);
    assert!(
        file.exists(),
        "reading a corrupt provenance must not quarantine it"
    );
}

const SHA: &str = "3a66ac2200b5e4c1949c61f6555f3778c9b2c4b153c696ad8c27fc422f18f86d";

/// An extract command that reads its request on stdin, with `invoke` and
/// `install` as its tail.
fn reading(invoke: &str, install: &str) -> String {
    format!(
        "id = \"cli:tool\"\ncapabilities = [\"extract\"]\nexecution = \"sync\"\ndetect = {{ file = \"bundle.json\" }}\n[invoke]\n{invoke}\n{install}"
    )
}

/// An `[install]` with one build for `target` at `url`, `sha256` and `binary`.
fn install(target: &str, url: &str, sha256: &str, binary: &str) -> String {
    format!(
        "[install]\nversion = \"1.0.0\"\n[install.targets.{target}]\nurl = \"{url}\"\nsha256 = \"{sha256}\"\nbinary = \"{binary}\"\n"
    )
}

#[test]
fn a_command_reads_its_payload_on_stdin_and_nothing_else() {
    let manifest = parse(&reading(
        "command = \"tool json\"\nstdin = \"{payload}\"",
        "",
    ))
    .expect("stdin payload");
    assert_eq!(
        manifest.invoke.and_then(|invoke| invoke.stdin),
        Some(kernel::manifests::InvokeStdin::Payload)
    );
    let err = parse(&reading(
        "command = \"tool json\"\nstdin = \"{prompt}\"",
        "",
    ))
    .expect_err("another stdin is refused");
    assert!(err.to_string().contains("stdin"), "{err}");
}

#[test]
fn an_installed_command_pins_its_build_and_runs_it_as_bin() {
    let url = "https://example.com/tool-aarch64-apple-darwin.tar.gz";
    let manifest = parse(&reading(
        "command = \"{bin} json --bundle {model}\"\nstdin = \"{payload}\"",
        &install("aarch64-apple-darwin", url, SHA, "tool/tool"),
    ))
    .expect("a pinned build");
    let install = manifest.install.expect("install");
    assert_eq!(install.version, "1.0.0");
    let build = &install.targets["aarch64-apple-darwin"];
    assert_eq!(
        (build.url.as_str(), build.binary.as_str()),
        (url, "tool/tool")
    );
}

#[test]
fn an_install_that_could_run_something_else_is_refused() {
    let url = "https://example.com/tool.tar.gz";
    let bin = "command = \"{bin} json\"";
    let cases = [
        (
            "plain http",
            reading(
                bin,
                &install("x", "http://example.com/tool.tar.gz", SHA, "tool"),
            ),
        ),
        (
            "short sha256",
            reading(bin, &install("x", url, "abc123", "tool")),
        ),
        (
            "uppercase sha256",
            reading(bin, &install("x", url, &SHA.to_uppercase(), "tool")),
        ),
        (
            "binary outside the archive",
            reading(bin, &install("x", url, SHA, "../tool")),
        ),
        (
            "absolute binary",
            reading(bin, &install("x", url, SHA, "/bin/sh")),
        ),
        (
            "no targets",
            reading(bin, "[install]\nversion = \"1.0.0\"\n"),
        ),
        (
            "install not run",
            reading("command = \"tool json\"", &install("x", url, SHA, "tool")),
        ),
        ("bin without install", reading(bin, "")),
    ];
    for (name, text) in cases {
        assert!(parse(&text).is_err(), "expected rejection: {name}");
    }
}

#[test]
fn a_vm_runtime_cannot_also_install_a_command() {
    let text = format!(
        "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"{{bin}}\"\n[vm]\nimage = \"ghcr.io/acme/tool@sha256:abc123\"\n{}",
        install("x", "https://example.com/t.tar.gz", SHA, "t")
    );
    let err = parse(&text).expect_err("rejected");
    assert!(err.to_string().contains("[vm] and [install]"), "{err}");
}
