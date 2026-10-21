//! `.github/fetch-llama-cpp.sh`, which release builds and CI run, fetches for
//! each target dist builds the llama.cpp archive the pin says hedos carries
//! there, or hands Linux to `.github/build-llama-cpp.sh`, which builds it from
//! the pinned source. The build script checks what they make against the pin;
//! this keeps the names in the scripts in step with it, so a new pin that
//! forgets them fails here, not in a release.

use std::path::Path;

use kernel::engines::llama_cpp::{PINNED, built_from_source, bundled_variant};
use kernel::machine::Os;

fn repo_file(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn dist_targets() -> Vec<String> {
    let config = repo_file("dist-workspace.toml");
    let line = config
        .lines()
        .find(|line| line.trim_start().starts_with("targets ="))
        .expect("dist-workspace.toml names its targets");
    line.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[test]
fn every_release_target_fetches_or_builds_the_pinned_build_hedos_carries_there() {
    let fetch = repo_file(".github/fetch-llama-cpp.sh");
    let build = repo_file(".github/build-llama-cpp.sh");
    let base = PINNED.url("");
    assert!(
        fetch.contains(&format!("base={}", base.trim_end_matches('/'))),
        "fetch-llama-cpp.sh downloads from another release than {base}"
    );
    let targets = dist_targets();
    assert_eq!(targets.len(), 4, "{targets:?}");
    for target in targets {
        let os = if target.contains("apple-darwin") {
            Os::Macos
        } else if target.contains("linux") {
            Os::Linux
        } else {
            Os::Other
        };
        let arch = target.split('-').next().unwrap_or_default();
        let variant = bundled_variant(os, arch)
            .unwrap_or_else(|| panic!("hedos carries no llama.cpp for {target}"));
        if built_from_source(os) {
            assert!(
                fetch.contains(&target) && build.contains(&format!("{target})")),
                "{target} is not handed to build-llama-cpp.sh"
            );
            continue;
        }
        let asset = PINNED
            .asset(os, arch, variant)
            .unwrap_or_else(|| panic!("the pin has no {} build for {target}", variant.as_str()));
        let case = format!("{target}) file={} ;;", asset.file);
        assert!(fetch.contains(&case), "fetch-llama-cpp.sh lacks `{case}`");
    }
}

#[test]
fn the_linux_build_is_made_from_the_pinned_source() {
    let build = repo_file(".github/build-llama-cpp.sh");
    for (what, pinned) in [
        ("build", PINNED.build),
        ("commit", PINNED.source.commit),
        ("source_sha256", PINNED.source.sha256),
    ] {
        let line = format!("{what}={pinned}");
        assert!(build.contains(&line), "build-llama-cpp.sh lacks `{line}`");
    }
    assert!(
        PINNED
            .source_url()
            .ends_with(&format!("/{}.tar.gz", PINNED.build))
            && build.contains("archive/refs/tags/$build.tar.gz"),
        "build-llama-cpp.sh fetches the source from elsewhere than {}",
        PINNED.source_url()
    );
}

#[test]
fn the_release_workflow_runs_the_setup_steps() {
    let release = repo_file(".github/workflows/release.yml");
    assert!(
        release.contains(".github/fetch-llama-cpp.sh"),
        "release.yml is stale: run `dist generate`"
    );
}
