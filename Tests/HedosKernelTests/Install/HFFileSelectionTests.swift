import Foundation
import Testing

@testable import HedosKernel

private func sibling(_ path: String, bytes: Int64 = 100) -> HFSibling {
    HFSibling(rfilename: path, size: bytes)
}

struct HFFileSelectionTests {
    @Test func ggufRepoPicksOnePreferredQuantPlusCompanions() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model-Q2_K.gguf", bytes: 1000),
            sibling("model-Q4_K_M.gguf", bytes: 2000),
            sibling("model-Q8_0.gguf", bytes: 4000),
            sibling("mmproj-model-f16.gguf", bytes: 500),
            sibling("config.json", bytes: 100),
            sibling("README.md", bytes: 10),
        ])
        let names = Set(selection.map(\.rfilename))
        #expect(names == ["model-Q4_K_M.gguf", "mmproj-model-f16.gguf", "config.json"])
    }

    @Test func ggufRepoWithoutPreferredQuantPicksSmallest() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model-Q2_K.gguf", bytes: 1000),
            sibling("model-Q3_K_L.gguf", bytes: 1500),
        ])
        #expect(selection.map(\.rfilename) == ["model-Q2_K.gguf"])
    }

    @Test func ggufShardGroupsStayWhole() {
        let selection = HFFileSelection.select(siblings: [
            sibling("big-q4_0-00001-of-00002.gguf", bytes: 1000),
            sibling("big-q4_0-00002-of-00002.gguf", bytes: 900),
            sibling("big-q8_0.gguf", bytes: 4000),
        ])
        let names = Set(selection.map(\.rfilename))
        #expect(
            names == ["big-q4_0-00001-of-00002.gguf", "big-q4_0-00002-of-00002.gguf"])
    }

    @Test func incompleteShardSetOnTheHubIsSkippedForACompleteQuant() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model-Q4_K_M-00001-of-00003.gguf", bytes: 1000),
            sibling("model-Q4_K_M-00003-of-00003.gguf", bytes: 900),
            sibling("model-Q8_0.gguf", bytes: 4000),
        ])
        #expect(selection.map(\.rfilename) == ["model-Q8_0.gguf"])
    }

    @Test func shardGroupsWithDifferentTotalsNeverMerge() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model-q4_0-00001-of-00002.gguf", bytes: 1000),
            sibling("model-q4_0-00002-of-00003.gguf", bytes: 900),
            sibling("model-q8_0.gguf", bytes: 4000),
        ])
        #expect(selection.map(\.rfilename) == ["model-q8_0.gguf"])
    }

    @Test func diffusersRepoKeepsTreeAndDropsShadowedWeights() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model_index.json"),
            sibling("unet/diffusion_pytorch_model.safetensors", bytes: 5000),
            sibling("unet/diffusion_pytorch_model.bin", bytes: 5000),
            sibling("unet/config.json"),
            sibling("vae/diffusion_pytorch_model.fp16.safetensors", bytes: 2000),
            sibling("vae/diffusion_pytorch_model.safetensors", bytes: 4000),
            sibling("vae/config.json"),
            sibling("onnx/model.onnx", bytes: 9000),
            sibling("text_encoder/model.safetensors", bytes: 3000),
            sibling(".gitattributes", bytes: 1),
        ])
        let names = Set(selection.map(\.rfilename))
        #expect(
            names == [
                "model_index.json",
                "unet/diffusion_pytorch_model.safetensors",
                "unet/config.json",
                "vae/diffusion_pytorch_model.safetensors",
                "vae/config.json",
                "text_encoder/model.safetensors",
            ])
    }

    @Test func incompleteShardsWithOnlyAProjectorRefuseSelection() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model-Q4_K_M-00001-of-00003.gguf", bytes: 1000),
            sibling("mmproj-model-f16.gguf", bytes: 500),
            sibling("config.json", bytes: 100),
        ])
        #expect(selection.isEmpty)
    }

    @Test func armRepackedVariantsDoNotSatisfyTheBaseQuant() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model-Q4_0_4_4.gguf", bytes: 2000),
            sibling("model-Q4_0.gguf", bytes: 2100),
        ])
        #expect(selection.map(\.rfilename) == ["model-Q4_0.gguf"])
    }

    @Test func bf16DoesNotSatisfyTheF16Preference() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model-bf16.gguf", bytes: 16_000),
            sibling("model-q2_k.gguf", bytes: 3000),
        ])
        #expect(selection.map(\.rfilename) == ["model-q2_k.gguf"])
    }

    @Test func quantDirectoriesSatisfyThePreferenceAtTokenBoundaries() {
        let selection = HFFileSelection.select(siblings: [
            sibling("Q4_K_M/model-plain.gguf", bytes: 5000),
            sibling("Q8_0/model-plain.gguf", bytes: 9000),
            sibling("IQ1_S/model-plain.gguf", bytes: 1000),
        ])
        #expect(selection.map(\.rfilename) == ["Q4_K_M/model-plain.gguf"])
    }

    @Test func bf16DirectoryDoesNotSatisfyTheF16Preference() {
        let selection = HFFileSelection.select(siblings: [
            sibling("BF16/model-plain.gguf", bytes: 16_000),
            sibling("model-q2_k.gguf", bytes: 3000),
        ])
        #expect(selection.map(\.rfilename) == ["model-q2_k.gguf"])
    }

    @Test func diffusersRepoDropsRootSingleFileCheckpoints() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model_index.json"),
            sibling("v1-5-pruned.safetensors", bytes: 7000),
            sibling("v1-5-pruned-emaonly.safetensors", bytes: 4000),
            sibling("v1-5-pruned-emaonly.ckpt", bytes: 4000),
            sibling("unet/diffusion_pytorch_model.safetensors", bytes: 3000),
            sibling("unet/config.json"),
            sibling("scheduler/scheduler_config.json"),
        ])
        let names = Set(selection.map(\.rfilename))
        #expect(
            names == [
                "model_index.json",
                "unet/diffusion_pytorch_model.safetensors",
                "unet/config.json",
                "scheduler/scheduler_config.json",
            ])
    }

    @Test func diffusersRepoWithOnlyRootCheckpointsKeepsThem() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model_index.json"),
            sibling("v1-5-pruned.safetensors", bytes: 7000),
            sibling("v1-5-pruned.ckpt", bytes: 7000),
            sibling("scheduler/scheduler_config.json"),
        ])
        let names = Set(selection.map(\.rfilename))
        #expect(
            names == [
                "model_index.json",
                "v1-5-pruned.safetensors",
                "scheduler/scheduler_config.json",
            ])
    }

    @Test func transformersRepoTakesSafetensorsAndConfigs() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model-00001-of-00002.safetensors", bytes: 5000),
            sibling("model-00002-of-00002.safetensors", bytes: 5000),
            sibling("model.safetensors.index.json", bytes: 50),
            sibling("pytorch_model.bin", bytes: 10000),
            sibling("config.json"),
            sibling("generation_config.json"),
            sibling("tokenizer.model", bytes: 500),
            sibling("banner.png", bytes: 100),
            sibling("README.md", bytes: 10),
        ])
        let names = Set(selection.map(\.rfilename))
        #expect(
            names == [
                "model-00001-of-00002.safetensors",
                "model-00002-of-00002.safetensors",
                "model.safetensors.index.json",
                "config.json",
                "generation_config.json",
                "tokenizer.model",
            ])
    }

    @Test func transformersRepoFallsBackToPytorchBin() {
        let selection = HFFileSelection.select(siblings: [
            sibling("pytorch_model.bin", bytes: 10000),
            sibling("pytorch_model.bin.index.json", bytes: 50),
            sibling("config.json"),
            sibling("flax_model.msgpack", bytes: 10000),
            sibling("tf_model.h5", bytes: 10000),
        ])
        let names = Set(selection.map(\.rfilename))
        #expect(
            names == ["pytorch_model.bin", "pytorch_model.bin.index.json", "config.json"])
    }

    @Test func oversizedSupportFilesAreDropped() {
        let selection = HFFileSelection.select(siblings: [
            sibling("model.safetensors", bytes: 5000),
            sibling("config.json"),
            sibling("training_dump.jsonl", bytes: HFFileSelection.configCap + 1),
        ])
        let names = Set(selection.map(\.rfilename))
        #expect(names == ["model.safetensors", "config.json"])
    }
}
