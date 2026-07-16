import Foundation
import Testing

@testable import HedosKernel

struct InstallReferenceTests {
    @Test func huggingFaceAcceptsPlainReposAndLinks() {
        #expect(InstallReference.huggingFaceRepo(from: "org/repo") == "org/repo")
        #expect(
            InstallReference.huggingFaceRepo(
                from: "https://huggingface.co/thinkingmachines/Inkling")
                == "thinkingmachines/Inkling")
        #expect(
            InstallReference.huggingFaceRepo(from: "huggingface.co/org/repo/") == "org/repo")
        #expect(InstallReference.huggingFaceRepo(from: "hf.co/org/repo") == "org/repo")
        #expect(
            InstallReference.huggingFaceRepo(
                from: "https://huggingface.co/org/repo/tree/main") == "org/repo")
        #expect(
            InstallReference.huggingFaceRepo(
                from: "https://huggingface.co/org/repo?not-for-all-audiences=true")
                == "org/repo")
    }

    @Test func huggingFaceRejectsNonModelShapes() {
        #expect(InstallReference.huggingFaceRepo(from: "gemma3:4b") == nil)
        #expect(InstallReference.huggingFaceRepo(from: "single") == nil)
        #expect(InstallReference.huggingFaceRepo(from: "") == nil)
        #expect(InstallReference.huggingFaceRepo(from: "has space/repo") == nil)
        #expect(
            InstallReference.huggingFaceRepo(from: "https://huggingface.co/datasets/org/name")
                == nil)
        #expect(InstallReference.huggingFaceRepo(from: "a/b/c") == nil)
        #expect(
            InstallReference.huggingFaceRepo(from: "ollama.com/library/gemma3") == nil)
        #expect(InstallReference.huggingFaceRepo(from: "ftp://huggingface.co/org/repo") == nil)
    }

    @Test func ollamaAcceptsTagsAndLibraryLinks() {
        #expect(InstallReference.ollamaTag(from: "gemma3:4b") == "gemma3:4b")
        #expect(InstallReference.ollamaTag(from: "gemma3") == "gemma3")
        #expect(InstallReference.ollamaTag(from: "user/model:tag") == "user/model:tag")
        #expect(
            InstallReference.ollamaTag(from: "https://ollama.com/library/gemma3") == "gemma3")
        #expect(
            InstallReference.ollamaTag(from: "ollama.com/library/gemma3:12b") == "gemma3:12b")
        #expect(
            InstallReference.ollamaTag(from: "https://ollama.com/library/gemma3/tags")
                == "gemma3")
    }

    @Test func ollamaKeepsUserNamespacedLinksWhole() {
        #expect(
            InstallReference.ollamaTag(from: "https://ollama.com/nezahatkorkmaz/deepseek-v3")
                == "nezahatkorkmaz/deepseek-v3")
        #expect(
            InstallReference.ollamaTag(
                from: "https://ollama.com/nezahatkorkmaz/deepseek-v3/tags")
                == "nezahatkorkmaz/deepseek-v3")
        #expect(
            InstallReference.ollamaTag(from: "ollama.com/user/model:tag") == "user/model:tag")
        #expect(InstallReference.ollamaTag(from: "user/model") == nil)
    }

    @Test func ollamaInstallTagAcceptsNamespacedWithoutExplicitTag() {
        #expect(InstallReference.ollamaInstallTag(from: "user/model") == "user/model")
        #expect(InstallReference.ollamaInstallTag(from: "gemma3:4b") == "gemma3:4b")
        #expect(InstallReference.ollamaInstallTag(from: "a/b/c") == nil)
    }

    @Test func ollamaRejectsForeignShapes() {
        #expect(InstallReference.ollamaTag(from: "") == nil)
        #expect(InstallReference.ollamaTag(from: "has space") == nil)
        #expect(InstallReference.ollamaTag(from: "name:") == nil)
        #expect(InstallReference.ollamaTag(from: "a/b/c:d") == nil)
        #expect(InstallReference.ollamaTag(from: "ftp://ollama.com/library/x") == nil)
    }
}
