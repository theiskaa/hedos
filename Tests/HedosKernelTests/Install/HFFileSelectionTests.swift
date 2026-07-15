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
