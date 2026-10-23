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
        ("docs/README.md", Some(1)),
        ("notes.md", Some(1)),
        ("preview.png", Some(1)),
        ("weights.msgpack", Some(1)),
        ("flax_model.msgpack", Some(1)),
        ("tf_model.h5", Some(1)),
        (".gitattributes", Some(1)),
        ("onnx/model.onnx", Some(1)),
        ("logo/.hidden", Some(1)),
    ]));
    // The safetensors weight, the config and the root model card survive; the
    // card's front matter is read to identify the model.
    assert_eq!(
        names(&picked),
        set(&["model.safetensors", "config.json", "README.md"])
    );
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

#[test]
fn transformers_keeps_config_and_tokenizer_from_weightless_subdirectories() {
    // convaiinnovations/laya: the root checkpoint's config and tokenizer live one
    // level down, and two alternative checkpoints sit beside them with weights of
    // their own.
    let picked = select(&siblings(&[
        ("model.safetensors", Some(816 << 20)),
        ("rl_agent_config.json", Some(5)),
        ("modeling_laya.py", Some(5)),
        ("encoder/config.json", Some(5)),
        ("tokenizer/tokenizer.json", Some(5)),
        ("tokenizer/tokenizer_config.json", Some(5)),
        ("typed-decisions/model.safetensors", Some(816 << 20)),
        ("typed-decisions/encoder/config.json", Some(5)),
        ("multilingual/model.safetensors", Some(816 << 20)),
        ("multilingual/tokenizer/tokenizer.json", Some(5)),
        ("assets/logo-lockup.svg", Some(5)),
    ]));
    assert_eq!(
        names(&picked),
        set(&[
            "model.safetensors",
            "rl_agent_config.json",
            "modeling_laya.py",
            "encoder/config.json",
            "tokenizer/tokenizer.json",
            "tokenizer/tokenizer_config.json",
        ]),
        "the root checkpoint arrives whole, and the alternatives not at all"
    );
}

#[test]
fn transformers_takes_a_config_that_only_exists_nested() {
    let picked = select(&siblings(&[
        ("model.safetensors", Some(500)),
        ("encoder/config.json", Some(5)),
    ]));
    assert_eq!(
        names(&picked),
        set(&["model.safetensors", "encoder/config.json"])
    );
}

#[test]
fn transformers_leaves_an_alternative_checkpoints_support_files_behind() {
    let picked = select(&siblings(&[
        ("model.safetensors", Some(500)),
        ("config.json", Some(5)),
        ("fp32/model.safetensors", Some(2000)),
        ("fp32/config.json", Some(5)),
        ("fp32/tokenizer/tokenizer.json", Some(5)),
    ]));
    assert_eq!(names(&picked), set(&["model.safetensors", "config.json"]));
}

#[test]
fn transformers_selects_a_flat_repo_exactly_as_it_always_did() {
    let picked = select(&siblings(&[
        ("model-00001-of-00002.safetensors", Some(500)),
        ("model-00002-of-00002.safetensors", Some(500)),
        ("model.safetensors.index.json", Some(1)),
        ("config.json", Some(5)),
        ("tokenizer.json", Some(5)),
        ("tokenizer_config.json", Some(5)),
        ("pytorch_model.bin", Some(500)),
        ("adapter_model.bin", Some(500)),
        ("README.md", Some(1)),
    ]));
    assert_eq!(
        names(&picked),
        set(&[
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
            "model.safetensors.index.json",
            "config.json",
            "tokenizer.json",
            "tokenizer_config.json",
            "README.md",
        ])
    );
}

#[test]
fn a_whole_model_in_another_framework_is_not_a_support_file() {
    // Each of these is under the 100 MiB support cap and has no extension the
    // weight set recognises, so without an exclusion they ride along as support.
    let picked = select(&siblings(&[
        ("model.safetensors", Some(90 << 20)),
        ("config.json", Some(5)),
        ("rust_model.ot", Some(90 << 20)),
        ("model.onnx", Some(90 << 20)),
        ("model.tflite", Some(90 << 20)),
    ]));
    assert_eq!(names(&picked), set(&["model.safetensors", "config.json"]));
}

const GB: i64 = 1_000_000_000;

/// `(name, gigabytes)` as a repo listing reports them.
fn listing(files: &[(&str, f64)]) -> Vec<HFSibling> {
    files
        .iter()
        .map(|(name, gb)| HFSibling::new(*name, Some((gb * GB as f64) as i64)))
        .collect()
}

#[test]
fn a_speculative_draft_is_never_pulled_for_the_model() {
    // ggml-org/gpt-oss-20b-GGUF: the model matches no preferred quantization,
    // and its EAGLE3 draft is Q8_0.
    let picked = select(&listing(&[
        ("eagle3-gpt-oss-20b-BF16.gguf", 1.72),
        ("eagle3-gpt-oss-20b-Q8_0.gguf", 0.92),
        ("gpt-oss-20b-MXFP4.gguf", 12.11),
    ]));
    assert_eq!(names(&picked), set(&["gpt-oss-20b-MXFP4.gguf"]));
}

#[test]
fn ggml_orgs_repos_pull_the_model_and_one_projector() {
    // ggml-org/gemma-4-12B-it-GGUF.
    let picked = select(&listing(&[
        ("gemma-4-12B-it-BF16.gguf", 23.83),
        ("gemma-4-12B-it-Q4_0.gguf", 7.22),
        ("gemma-4-12B-it-Q8_0.gguf", 12.67),
        ("mmproj-gemma-4-12B-it-BF16.gguf", 0.18),
        ("mmproj-gemma-4-12B-it-Q8_0.gguf", 0.16),
        ("mtp-gemma-4-12B-it-BF16.gguf", 0.86),
        ("mtp-gemma-4-12B-it-Q4_0.gguf", 0.25),
        ("mtp-gemma-4-12B-it-Q8_0.gguf", 0.47),
    ]));
    assert_eq!(
        names(&picked),
        set(&[
            "gemma-4-12B-it-Q4_0.gguf",
            "mmproj-gemma-4-12B-it-Q8_0.gguf"
        ])
    );

    // ggml-org/Qwen3.8-27B-GGUF.
    let picked = select(&listing(&[
        ("Qwen3.8-27B-BF16.gguf", 53.81),
        ("Qwen3.8-27B-Q4_K_M.gguf", 18.97),
        ("Qwen3.8-27B-Q8_0.gguf", 28.60),
        ("dflash-Qwen3.8-27B-BF16.gguf", 3.86),
        ("dflash-Qwen3.8-27B-Q4_0.gguf", 1.09),
        ("dflash-Qwen3.8-27B-Q8_0.gguf", 2.06),
        ("mmproj-Qwen3.8-27B-BF16.gguf", 0.93),
        ("mmproj-Qwen3.8-27B-Q8_0.gguf", 0.63),
        ("mtp-Qwen3.8-27B-BF16.gguf", 5.95),
        ("mtp-Qwen3.8-27B-Q4_0.gguf", 1.68),
        ("mtp-Qwen3.8-27B-Q8_0.gguf", 3.16),
    ]));
    assert_eq!(
        names(&picked),
        set(&["Qwen3.8-27B-Q4_K_M.gguf", "mmproj-Qwen3.8-27B-Q8_0.gguf"])
    );
}

#[test]
fn of_several_projector_precisions_the_one_named_for_the_weights_else_the_smallest() {
    // unsloth/Qwen3.5-4B-GGUF: no projector is Q4_K_M, so the smallest.
    let picked = select(&listing(&[
        ("Qwen3.5-4B-Q4_K_M.gguf", 2.74),
        ("Qwen3.5-4B-Q8_0.gguf", 4.48),
        ("Qwen3.5-4B-UD-Q4_K_XL.gguf", 2.91),
        ("mmproj-BF16.gguf", 0.68),
        ("mmproj-F16.gguf", 0.67),
        ("mmproj-F32.gguf", 1.33),
    ]));
    assert_eq!(
        names(&picked),
        set(&["Qwen3.5-4B-Q4_K_M.gguf", "mmproj-F16.gguf"])
    );

    // A projector at the weights' own quantization wins over a smaller one.
    let picked = select(&listing(&[
        ("model-Q8_0.gguf", 4.0),
        ("mmproj-model-Q8_0.gguf", 0.6),
        ("mmproj-model-Q4_0.gguf", 0.3),
    ]));
    assert_eq!(
        names(&picked),
        set(&["model-Q8_0.gguf", "mmproj-model-Q8_0.gguf"])
    );
}

#[test]
fn qwens_own_repo_pulls_the_single_file_rather_than_its_shards() {
    // Qwen/Qwen2.5-Coder-7B-Instruct-GGUF (the q4_k_m files).
    let picked = select(&listing(&[
        ("qwen2.5-coder-7b-instruct-q4_k_m-00001-of-00002.gguf", 3.99),
        ("qwen2.5-coder-7b-instruct-q4_k_m-00002-of-00002.gguf", 0.69),
        ("qwen2.5-coder-7b-instruct-q4_k_m.gguf", 4.68),
        ("qwen2.5-coder-7b-instruct-q8_0.gguf", 8.10),
    ]));
    assert_eq!(
        names(&picked),
        set(&["qwen2.5-coder-7b-instruct-q4_k_m.gguf"])
    );
}

#[test]
fn weights_outside_the_preference_list_keep_the_projector_of_their_own_precision() {
    // BF16 weights keep the BF16 projector, as serving one already on disk
    // would choose it.
    let picked = select(&listing(&[
        ("model-BF16.gguf", 8.4),
        ("mmproj-model-BF16.gguf", 0.9),
        ("mmproj-model-Q8_0.gguf", 0.6),
    ]));
    assert_eq!(
        names(&picked),
        set(&["model-BF16.gguf", "mmproj-model-BF16.gguf"])
    );
}

#[test]
fn a_projector_beside_the_weights_or_at_the_root_wins_over_one_elsewhere() {
    // The shelf counts a projector into serving only beside the weights or at
    // the root, so one in another quantization's folder is the last resort.
    let picked = select(&listing(&[
        ("Q4_K_M/model-Q4_K_M.gguf", 4.0),
        ("Q8_0/model-Q8_0.gguf", 7.0),
        ("mmproj-model-F16.gguf", 0.9),
        ("Q8_0/mmproj-model-Q8_0.gguf", 0.6),
    ]));
    assert_eq!(
        names(&picked),
        set(&["Q4_K_M/model-Q4_K_M.gguf", "mmproj-model-F16.gguf"])
    );
}
