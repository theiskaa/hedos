//! Tests for the install plan types and the file classifiers.

use kernel::install::InstallProviderId;
use kernel::install::file_selection::{file_extension, is_weight_path};
use kernel::install::plan::{InstallPlan, InstallPlanFile};

#[test]
fn file_extension_takes_the_last_dot_and_lowercases() {
    assert_eq!(file_extension("model.SafeTensors"), "safetensors");
    assert_eq!(file_extension("a/b/model.gguf"), "gguf");
    assert_eq!(file_extension("archive.tar.gz"), "gz");
    // No extension, or only a leading dot (a dotfile), → empty.
    assert_eq!(file_extension("README"), "");
    assert_eq!(file_extension(".gitignore"), "");
    assert_eq!(file_extension("dir/plainfile"), "");
    // A trailing dot → empty extension.
    assert_eq!(file_extension("file."), "");
    // A dotfile inside a subdirectory DOES get an extension (the leading-dot rule
    // is byte-0 of the whole path, not of the last segment) — pin this quirk.
    assert_eq!(file_extension("dir/.hidden"), "hidden");
}

#[test]
fn is_weight_path_recognizes_weight_extensions() {
    for path in [
        "model.safetensors",
        "model.gguf",
        "pytorch_model.bin",
        "sd.ckpt",
        "weights.pt",
        "weights.pth",
        "shard/00001.gguf",
        // Extension matching is case-insensitive.
        "model.GGUF",
    ] {
        assert!(is_weight_path(path), "{path} should be a weight");
    }
    for path in ["config.json", "README.md", "tokenizer.model.txt", "noext"] {
        assert!(!is_weight_path(path), "{path} should not be a weight");
    }
}

#[test]
fn plan_file_reports_whether_it_is_a_weight() {
    assert!(InstallPlanFile::new("model.gguf", Some(1024)).is_weight());
    assert!(!InstallPlanFile::new("config.json", None).is_weight());
}

#[test]
fn install_plan_new_defaults_the_optional_fields() {
    let plan = InstallPlan::new(
        InstallProviderId::huggingface(),
        "org/model",
        "Org Model",
        "/cache/org/model",
    );
    assert_eq!(plan.provider, InstallProviderId::huggingface());
    assert_eq!(plan.reference, "org/model");
    assert_eq!(plan.display_name, "Org Model");
    assert_eq!(plan.destination, "/cache/org/model");
    assert_eq!(plan.revision, None);
    assert!(plan.files.is_empty());
    assert_eq!(plan.total_bytes, None);
    assert_eq!(plan.remaining_bytes, None);
    assert!(!plan.requires_auth);
}
