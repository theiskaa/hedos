//! `.github/fetch-llama-cpp.sh`, which release builds and CI run, fetches for
//! each target dist builds the llama.cpp archive the pin says hedos carries
//! there, from whoever publishes it: llama.cpp for macOS, hedos for Linux,
//! whose archives `.github/build-llama-cpp.sh` makes from the pinned source.
//! The build script checks what they fetch against the pin; this keeps the
//! names in the scripts in step with it, so a new pin that forgets them fails
//! here, not in a release.

use std::path::Path;

use kernel::engines::llama_cpp::{PINNED, Publisher, bundled_variant};
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
fn every_release_target_fetches_the_pinned_build_hedos_carries_there() {
    let fetch = repo_file(".github/fetch-llama-cpp.sh");
    for (name, publisher) in [("llama", Publisher::LlamaCpp), ("hedos", Publisher::Hedos)] {
        let base = PINNED.url(publisher, "");
        let line = format!("{name}={}", base.trim_end_matches('/'));
        assert!(fetch.contains(&line), "fetch-llama-cpp.sh lacks `{line}`");
    }
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
        let asset = PINNED
            .asset(os, arch, variant)
            .unwrap_or_else(|| panic!("the pin has no {} build for {target}", variant.as_str()));
        let base = match asset.publisher {
            Publisher::LlamaCpp => "llama",
            Publisher::Hedos => "hedos",
        };
        let case = format!("{target}) base=${base}; file={} ;;", asset.file);
        assert!(fetch.contains(&case), "fetch-llama-cpp.sh lacks `{case}`");
    }
}

#[test]
fn hedos_s_own_builds_are_the_archives_build_llama_cpp_makes() {
    let build = repo_file(".github/build-llama-cpp.sh");
    assert!(
        build.contains(r#"archive="llama-$build-bin-linux-${target%%-*}.tar.gz""#),
        "build-llama-cpp.sh names its archive differently"
    );
    for asset in PINNED.assets {
        if asset.publisher != Publisher::Hedos {
            continue;
        }
        assert_eq!(asset.os, Os::Linux, "{}", asset.file);
        assert_eq!(
            asset.file,
            format!("llama-{}-bin-linux-{}.tar.gz", PINNED.build, asset.arch)
        );
        let target = format!("{}-unknown-linux-gnu)", asset.arch);
        assert!(build.contains(&target), "build-llama-cpp.sh lacks {target}");
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
