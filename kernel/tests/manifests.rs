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
