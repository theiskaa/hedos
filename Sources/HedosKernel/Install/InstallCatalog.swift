import Foundation

public struct InstallCategory: RawRepresentable, Codable, Hashable, Sendable,
    ExpressibleByStringLiteral
{
    public let rawValue: String
    public init(rawValue: String) { self.rawValue = rawValue }
    public init(stringLiteral value: String) { self.rawValue = value }

    public static let chat = InstallCategory(rawValue: "chat")
    public static let code = InstallCategory(rawValue: "code")
    public static let voice = InstallCategory(rawValue: "voice")
    public static let image = InstallCategory(rawValue: "image")
}

public struct InstallCatalogEntry: Sendable, Hashable, Identifiable {
    public let provider: InstallProviderID
    public let reference: String
    public let name: String
    public let blurb: String
    public let sizeGB: Double
    public let category: InstallCategory

    public init(
        provider: InstallProviderID, reference: String, name: String, blurb: String,
        sizeGB: Double, category: InstallCategory
    ) {
        self.provider = provider
        self.reference = reference
        self.name = name
        self.blurb = blurb
        self.sizeGB = sizeGB
        self.category = category
    }

    public var id: String { "\(provider.rawValue)|\(reference)" }

    public func fit(totalMemoryBytes: UInt64) -> FitVerdict.Assessment? {
        FitVerdict.assess(
            footprintMB: Int(sizeGB * 1024), totalMemoryBytes: totalMemoryBytes)
    }
}

public enum InstallCatalog {
    public static let entries: [InstallCatalogEntry] = [
        InstallCatalogEntry(
            provider: .ollama, reference: "gemma3:1b", name: "gemma3:1b",
            blurb: "Tiny and instant. Always fits.", sizeGB: 0.8, category: .chat),
        InstallCatalogEntry(
            provider: .ollama, reference: "llama3.2:3b", name: "llama3.2:3b",
            blurb: "Fast general chat on modest memory.", sizeGB: 2, category: .chat),
        InstallCatalogEntry(
            provider: .ollama, reference: "gemma3:4b", name: "gemma3:4b",
            blurb: "Fast everyday chat. Runs comfortably on any Mac.", sizeGB: 3.3,
            category: .chat),
        InstallCatalogEntry(
            provider: .ollama, reference: "gemma3:12b", name: "gemma3:12b",
            blurb: "Stronger reasoning, still nimble.", sizeGB: 8.1, category: .chat),
        InstallCatalogEntry(
            provider: .ollama, reference: "gemma3:27b", name: "gemma3:27b",
            blurb: "Flagship reasoning with room to spare on a big Mac.", sizeGB: 17,
            category: .chat),
        InstallCatalogEntry(
            provider: .ollama, reference: "llama3.3:70b", name: "llama3.3:70b",
            blurb: "The big one, at Q4. Leaves a little headroom, not much.", sizeGB: 40,
            category: .chat),
        InstallCatalogEntry(
            provider: .ollama, reference: "qwen2.5-coder:7b", name: "qwen2.5-coder:7b",
            blurb: "Everyday coding help that fits most Macs.", sizeGB: 4.7, category: .code),
        InstallCatalogEntry(
            provider: .ollama, reference: "qwen2.5-coder:14b", name: "qwen2.5-coder:14b",
            blurb: "Strong local coding model with a large context.", sizeGB: 9,
            category: .code),
        InstallCatalogEntry(
            provider: .ollama, reference: "deepseek-coder-v2:16b",
            name: "deepseek-coder-v2:16b",
            blurb: "Sharp on repository-scale edits and refactors.", sizeGB: 9.4,
            category: .code),
        InstallCatalogEntry(
            provider: .huggingface, reference: "hexgrad/Kokoro-82M", name: "kokoro-82m",
            blurb: "Tiny, warm text-to-speech. Instant on any Mac.", sizeGB: 0.3,
            category: .voice),
        InstallCatalogEntry(
            provider: .huggingface, reference: "openai/whisper-large-v3",
            name: "whisper-large-v3",
            blurb: "Best-in-class speech-to-text for dictation.", sizeGB: 1.5,
            category: .voice),
        InstallCatalogEntry(
            provider: .huggingface, reference: "black-forest-labs/FLUX.1-schnell",
            name: "flux.1-schnell",
            blurb: "Quick, striking image generation in a few steps.", sizeGB: 24,
            category: .image),
        InstallCatalogEntry(
            provider: .huggingface, reference: "stabilityai/stable-diffusion-xl-base-1.0",
            name: "sdxl",
            blurb: "Dependable, well-supported image workhorse.", sizeGB: 7,
            category: .image),
    ]

    public static func recommended(
        category: InstallCategory? = nil, ramGB: Int,
        providers: Set<InstallProviderID>? = nil
    ) -> [InstallCatalogEntry] {
        recommended(
            category: category, totalMemoryBytes: UInt64(max(ramGB, 1)) << 30,
            providers: providers)
    }

    public static func recommended(
        category: InstallCategory? = nil, totalMemoryBytes: UInt64,
        providers: Set<InstallProviderID>? = nil
    ) -> [InstallCatalogEntry] {
        var scoped = entries
        if let category {
            scoped = scoped.filter { $0.category == category }
        }
        if let providers {
            scoped = scoped.filter { providers.contains($0.provider) }
        }
        let fitting =
            scoped
            .filter { $0.fit(totalMemoryBytes: totalMemoryBytes)?.verdict == .runsWell }
            .sorted { ($0.sizeGB, $0.reference) < ($1.sizeGB, $1.reference) }
        if fitting.isEmpty {
            return scoped.min { ($0.sizeGB, $0.reference) < ($1.sizeGB, $1.reference) }
                .map { [$0] } ?? []
        }
        return Array(fitting.suffix(3))
    }
}
