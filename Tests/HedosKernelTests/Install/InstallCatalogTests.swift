import Foundation
import Testing

@testable import HedosKernel

struct InstallCatalogTests {
    @Test func entriesHaveUniqueIDs() {
        let ids = InstallCatalog.entries.map(\.id)
        #expect(Set(ids).count == ids.count)
    }

    @Test func huggingFaceReferencesAreRepoShaped() {
        for entry in InstallCatalog.entries where entry.provider == .huggingface {
            #expect(entry.reference.split(separator: "/").count == 2)
            #expect(!entry.reference.contains("://"))
        }
    }

    @Test func ollamaReferencesAreTagShaped() {
        for entry in InstallCatalog.entries where entry.provider == .ollama {
            #expect(entry.reference.contains(":"))
            #expect(!entry.reference.contains("/"))
        }
    }

    @Test func recommendedRespectsRAM() {
        let small = InstallCatalog.recommended(ramGB: 8, providers: [.ollama])
        #expect(!small.isEmpty)
        #expect(small.allSatisfy { $0.sizeGB <= 8 * 0.6 })
        let large = InstallCatalog.recommended(ramGB: 128, providers: [.ollama])
        #expect(large.count == 3)
        #expect(large.contains { $0.sizeGB >= 17 })
    }

    @Test func recommendedFallsBackToSmallestWhenNothingFits() {
        let picks = InstallCatalog.recommended(category: .image, ramGB: 1)
        #expect(picks.count == 1)
        #expect(picks.first?.name == "sdxl")
    }

    @Test func recommendedFiltersByCategory() {
        let picks = InstallCatalog.recommended(category: .code, ramGB: 64)
        #expect(!picks.isEmpty)
        #expect(picks.allSatisfy { $0.category == .code })
    }

    @Test func fitUsesFitVerdictVocabulary() {
        let entry = InstallCatalog.entries.first { $0.reference == "gemma3:4b" }
        let verdict = entry?.fit(totalMemoryBytes: 16 << 30)?.verdict
        #expect(verdict == .runsWell)
        let cramped = entry?.fit(totalMemoryBytes: 4 << 30)?.verdict
        #expect(cramped == .tooLarge)
    }
}
