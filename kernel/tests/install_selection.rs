//! Tests for Hugging Face repo file selection (`HFFileSelection::select`).

use std::collections::BTreeSet;

use kernel::install::file_selection::{HFSibling, select};

fn siblings(entries: &[(&str, Option<i64>)]) -> Vec<HFSibling> {
    entries
        .iter()
        .map(|(path, bytes)| HFSibling::new(*path, *bytes))
        .collect()
}

fn names(selected: &[HFSibling]) -> BTreeSet<String> {
    selected.iter().map(|s| s.rfilename.clone()).collect()
}

fn set(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| (*p).to_owned()).collect()
}

#[test]
fn eligibility_drops_docs_images_hidden_and_excluded_dirs() {
    let picked = select(&siblings(&[
        ("model.safetensors", Some(100)),
        ("config.json", Some(10)),
        ("README.md", Some(1)),
        ("preview.png", Some(1)),
        ("weights.msgpack", Some(1)),
        ("flax_model.msgpack", Some(1)),
        ("tf_model.h5", Some(1)),
        (".gitattributes", Some(1)),
        ("onnx/model.onnx", Some(1)),
        ("logo/.hidden", Some(1)),
    ]));
    // Only the safetensors weight and the config survive.
    assert_eq!(names(&picked), set(&["model.safetensors", "config.json"]));
}

#[test]
fn gguf_selection_prefers_the_best_quant_and_keeps_companions() {
    let picked = select(&siblings(&[
        ("model-q8_0.gguf", Some(800)),
        ("model-q4_k_m.gguf", Some(400)),
        ("model-q4_0.gguf", Some(400)),
        ("config.json", Some(10)),
        ("huge.json", Some(20 << 20)), // over the 10 MiB companion cap → dropped
    ]));
    // The q4_k_m group wins; the small config is a companion; the huge file is out.
    assert_eq!(names(&picked), set(&["model-q4_k_m.gguf", "config.json"]));
}

#[test]
fn a_single_unquantized_gguf_is_taken_with_companions() {
    let picked = select(&siblings(&[
        ("model.gguf", Some(500)),
        ("tokenizer.json", Some(5)),
    ]));
    assert_eq!(names(&picked), set(&["model.gguf", "tokenizer.json"]));
}

#[test]
fn a_complete_shard_set_is_taken_but_an_incomplete_one_is_dropped() {
    let complete = select(&siblings(&[
        ("model-00001-of-00002.gguf", Some(500)),
        ("model-00002-of-00002.gguf", Some(500)),
    ]));
    assert_eq!(
        names(&complete),
        set(&["model-00001-of-00002.gguf", "model-00002-of-00002.gguf"])
    );

    // A missing shard → no complete group → nothing selected.
    let incomplete = select(&siblings(&[("model-00001-of-00002.gguf", Some(500))]));
    assert!(incomplete.is_empty());
}

#[test]
fn an_mmproj_projector_rides_along_with_the_chosen_group() {
    let picked = select(&siblings(&[
        ("model-q4_k_m.gguf", Some(400)),
        ("mmproj-model-f16.gguf", Some(50)),
        ("config.json", Some(10)),
    ]));
    // mmproj is not a weight candidate (excluded), but is added alongside.
    assert_eq!(
        names(&picked),
        set(&["model-q4_k_m.gguf", "mmproj-model-f16.gguf", "config.json"])
    );
}

#[test]
fn quant_matching_respects_word_boundaries() {
    // `q4_0` glued to a letter is NOT a quant match, so this falls to smallest-bytes.
    let picked = select(&siblings(&[
        ("modelq4_0.gguf", Some(999)),
        ("other.gguf", Some(100)),
    ]));
    // Neither has a boundaried quant token, so the smallest group wins.
    assert_eq!(names(&picked), set(&["other.gguf"]));
}

#[test]
fn diffusers_drops_bin_twins_and_fp16_variants() {
    let picked = select(&siblings(&[
        ("model_index.json", Some(1)),
        ("unet/diffusion_pytorch_model.safetensors", Some(500)),
        ("unet/diffusion_pytorch_model.bin", Some(500)), // twin of the safetensors → dropped
        ("vae/model.fp16.safetensors", Some(100)),
        ("vae/model.safetensors", Some(200)), // plain form present → fp16 dropped
        ("config.json", Some(5)),
    ]));
    let result = names(&picked);
    assert!(result.contains("unet/diffusion_pytorch_model.safetensors"));
    assert!(!result.contains("unet/diffusion_pytorch_model.bin"));
    assert!(result.contains("vae/model.safetensors"));
    assert!(!result.contains("vae/model.fp16.safetensors"));
    assert!(result.contains("config.json"));
    assert!(result.contains("model_index.json"));
}

#[test]
fn transformers_takes_safetensors_then_falls_back_to_pytorch() {
    // Safetensors present → it is the weight set; pytorch is ignored.
    let with_st = select(&siblings(&[
        ("model.safetensors", Some(500)),
        ("pytorch_model.bin", Some(500)),
        ("config.json", Some(5)),
        ("tokenizer.json", Some(5)),
    ]));
    let result = names(&with_st);
    assert!(result.contains("model.safetensors"));
    assert!(!result.contains("pytorch_model.bin"));
    assert!(result.contains("config.json"));
    assert!(result.contains("tokenizer.json"));

    // No safetensors → the pytorch bin set is used.
    let pytorch = select(&siblings(&[
        ("pytorch_model.bin", Some(500)),
        ("config.json", Some(5)),
    ]));
    assert_eq!(names(&pytorch), set(&["pytorch_model.bin", "config.json"]));
}

#[test]
fn transformers_support_files_respect_the_config_cap() {
    let picked = select(&siblings(&[
        ("model.safetensors", Some(500)),
        ("config.json", Some(5)),
        ("giant_support.json", Some(200 << 20)), // over the 100 MiB config cap → dropped
    ]));
    let result = names(&picked);
    assert!(result.contains("model.safetensors"));
    assert!(result.contains("config.json"));
    assert!(!result.contains("giant_support.json"));
}

#[test]
fn diffusers_drops_a_root_weight_when_the_tree_carries_weights() {
    let picked = select(&siblings(&[
        ("model_index.json", Some(1)),
        ("unet/diffusion_pytorch_model.safetensors", Some(500)),
        ("model.safetensors", Some(400)), // a ROOT weight → dropped because the tree has weights
        ("model.non_ema.safetensors", Some(100)), // non_ema variant... but plain root is dropped anyway
        ("config.json", Some(5)),
    ]));
    let result = names(&picked);
    assert!(result.contains("unet/diffusion_pytorch_model.safetensors"));
    assert!(!result.contains("model.safetensors"));
    assert!(result.contains("config.json"));
}

#[test]
fn diffusers_drops_ckpt_twins_and_non_ema_variants() {
    let picked = select(&siblings(&[
        ("model_index.json", Some(1)),
        ("unet/model.safetensors", Some(500)),
        ("unet/model.ckpt", Some(500)), // ckpt twin of the safetensors → dropped
        ("vae/weights.non_ema.safetensors", Some(100)),
        ("vae/weights.safetensors", Some(200)), // plain form → non_ema dropped
    ]));
    let result = names(&picked);
    assert!(!result.contains("unet/model.ckpt"));
    assert!(result.contains("unet/model.safetensors"));
    assert!(!result.contains("vae/weights.non_ema.safetensors"));
    assert!(result.contains("vae/weights.safetensors"));
}

#[test]
fn an_mmproj_only_repo_selects_nothing() {
    // A projector with no actual weight group → no candidate → empty selection.
    let picked = select(&siblings(&[("mmproj-model-f16.gguf", Some(50))]));
    assert!(picked.is_empty());
}

#[test]
fn sharded_weights_in_a_subdirectory_group_correctly() {
    let picked = select(&siblings(&[
        ("gguf/model-00001-of-00002.gguf", Some(500)),
        ("gguf/model-00002-of-00002.gguf", Some(500)),
    ]));
    assert_eq!(
        names(&picked),
        set(&[
            "gguf/model-00001-of-00002.gguf",
            "gguf/model-00002-of-00002.gguf"
        ])
    );
}

#[test]
fn among_equal_quant_groups_the_first_in_sorted_order_wins() {
    // Two q4_k_m groups in different directories → directory `a` sorts before `b`.
    let picked = select(&siblings(&[
        ("b/model-q4_k_m.gguf", Some(400)),
        ("a/model-q4_k_m.gguf", Some(400)),
    ]));
    assert_eq!(names(&picked), set(&["a/model-q4_k_m.gguf"]));
}

#[test]
fn transformers_falls_back_to_pytorch_when_only_a_safetensors_index_exists() {
    let picked = select(&siblings(&[
        ("model.safetensors.index.json", Some(1)), // an index but no real .safetensors
        ("pytorch_model-00001-of-00002.bin", Some(500)),
        ("pytorch_model-00002-of-00002.bin", Some(500)),
        ("pytorch_model.bin.index.json", Some(1)),
        ("config.json", Some(5)),
    ]));
    let result = names(&picked);
    assert!(result.contains("pytorch_model-00001-of-00002.bin"));
    assert!(result.contains("pytorch_model.bin.index.json"));
    // The safetensors index (no real weight) is not selected.
    assert!(!result.contains("model.safetensors.index.json"));
    assert!(result.contains("config.json"));
}

#[test]
fn transformers_takes_a_sharded_safetensors_set() {
    let picked = select(&siblings(&[
        ("model-00001-of-00002.safetensors", Some(500)),
        ("model-00002-of-00002.safetensors", Some(500)),
        ("model.safetensors.index.json", Some(1)),
    ]));
    assert_eq!(
        names(&picked),
        set(&[
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
            "model.safetensors.index.json",
        ])
    );
}

#[test]
fn a_lone_non_pytorch_root_bin_is_dropped() {
    // `adapter_model.bin` isn't in the pytorch weight set, and a weight is excluded
    // from the support set — so it drops entirely.
    let picked = select(&siblings(&[
        ("adapter_model.bin", Some(500)),
        ("config.json", Some(5)),
    ]));
    assert_eq!(names(&picked), set(&["config.json"]));
}

#[test]
fn weights_under_other_excluded_directories_are_dropped() {
    let picked = select(&siblings(&[
        ("model.safetensors", Some(500)),
        ("openvino/model.safetensors", Some(1)),
        ("coreml/model.safetensors", Some(1)),
        ("config.json", Some(5)),
    ]));
    let result = names(&picked);
    assert!(!result.contains("openvino/model.safetensors"));
    assert!(!result.contains("coreml/model.safetensors"));
    assert!(result.contains("model.safetensors"));
}

#[test]
fn a_companion_with_unknown_size_is_kept_at_the_cap_boundary() {
    let picked = select(&siblings(&[
        ("model.gguf", Some(500)),
        ("cap_exact.json", Some(10 << 20)), // exactly at the 10 MiB cap → kept
        ("cfg.json", None),                 // unknown size → treated as 0 → kept
    ]));
    let result = names(&picked);
    assert!(result.contains("cap_exact.json"));
    assert!(result.contains("cfg.json"));
}
