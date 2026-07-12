import AppKit
import HedosKernel
import SwiftUI

enum SuggestionSource {
    case ollama
    case huggingface

    var badge: String {
        switch self {
        case .ollama: "OL"
        case .huggingface: "HF"
        }
    }
}

enum SuggestionCategory: String, CaseIterable, Identifiable {
    case chat = "Chat"
    case code = "Code"
    case voice = "Voice"
    case image = "Image"

    var id: String { rawValue }
}

enum SuggestionFit {
    case easy
    case good
    case tight
    case over

    var label: String {
        switch self {
        case .easy: "fits easily"
        case .good: "good fit"
        case .tight: "tight fit"
        case .over: "too large"
        }
    }
}

struct Suggestion: Identifiable {
    let id = UUID()
    let name: String
    let source: SuggestionSource
    let category: SuggestionCategory
    let blurb: String
    let sizeGB: Double
    let reference: String

    static let catalog: [Suggestion] = [
        Suggestion(
            name: "gemma3:4b", source: .ollama, category: .chat,
            blurb: "Fast everyday chat. Runs comfortably on any Mac.",
            sizeGB: 3.3, reference: "gemma3:4b"),
        Suggestion(
            name: "gemma3:27b", source: .ollama, category: .chat,
            blurb: "Flagship reasoning with room to spare on a big Mac.",
            sizeGB: 17, reference: "gemma3:27b"),
        Suggestion(
            name: "llama3.3:70b", source: .ollama, category: .chat,
            blurb: "The big one, at Q4. Leaves a little headroom, not much.",
            sizeGB: 40, reference: "llama3.3:70b"),
        Suggestion(
            name: "qwen2.5-coder:14b", source: .ollama, category: .code,
            blurb: "Strong local coding model with a large context.",
            sizeGB: 9, reference: "qwen2.5-coder:14b"),
        Suggestion(
            name: "deepseek-coder-v2:16b", source: .ollama, category: .code,
            blurb: "Sharp on repository-scale edits and refactors.",
            sizeGB: 9.4, reference: "deepseek-coder-v2:16b"),
        Suggestion(
            name: "kokoro-82m", source: .huggingface, category: .voice,
            blurb: "Tiny, warm text-to-speech. Instant on any Mac.",
            sizeGB: 0.3, reference: "https://huggingface.co/hexgrad/Kokoro-82M"),
        Suggestion(
            name: "whisper-large-v3", source: .huggingface, category: .voice,
            blurb: "Best-in-class speech-to-text for dictation.",
            sizeGB: 1.5, reference: "https://huggingface.co/openai/whisper-large-v3"),
        Suggestion(
            name: "flux.1-schnell", source: .huggingface, category: .image,
            blurb: "Quick, striking image generation in a few steps.",
            sizeGB: 24, reference: "https://huggingface.co/black-forest-labs/FLUX.1-schnell"),
        Suggestion(
            name: "sdxl", source: .huggingface, category: .image,
            blurb: "Dependable, well-supported image workhorse.",
            sizeGB: 7, reference: "https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0"),
    ]
}

struct HardwareProfile {
    let chip: String
    let ramGB: Int

    static let current = HardwareProfile()

    init() {
        ramGB = max(1, Int(ProcessInfo.processInfo.physicalMemory / 1_073_741_824))
        chip = HardwareProfile.readChip()
    }

    var summary: String {
        "\(chip) · \(ramGB) GB unified"
    }

    func fit(_ sizeGB: Double) -> SuggestionFit {
        let ratio = sizeGB / Double(ramGB)
        if ratio < 0.35 { return .easy }
        if ratio < 0.6 { return .good }
        if ratio < 0.9 { return .tight }
        return .over
    }

    private static func readChip() -> String {
        var size = 0
        sysctlbyname("machdep.cpu.brand_string", nil, &size, nil, 0)
        guard size > 0 else { return "Apple Silicon" }
        var buffer = [CChar](repeating: 0, count: size)
        sysctlbyname("machdep.cpu.brand_string", &buffer, &size, nil, 0)
        let value = String(cString: buffer)
        return value.isEmpty ? "Apple Silicon" : value
    }
}

struct FirstRunDiscovery: View {
    @Bindable var shell: ShellModel
    @State private var category: SuggestionCategory = .chat

    private let hardware = HardwareProfile.current

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.pane) {
            invitation
            suggestions
        }
    }

    private var invitation: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            Text("hedos never downloads behind your back. Point it at your disk and it finds the models you already have — or start from a recommendation tuned to your hardware.")
                .font(Design.readingBody)
                .lineSpacing(Design.readingLineSpacing)
                .foregroundStyle(Design.inkSoft)
                .frame(maxWidth: Design.Column.prose, alignment: .leading)
            Text(hardware.summary.uppercased())
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.accentText)
                .padding(.horizontal, Design.Space.l)
                .padding(.vertical, Design.Space.s)
                .overlay(
                    RoundedRectangle.soft(Design.Radius.control)
                        .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            HStack(spacing: Design.Space.m) {
                Button("Scan this Mac") {
                    Task { await shell.library.rescan() }
                }
                .buttonStyle(InkButtonStyle())
                .disabled(shell.library.isScanning)
                Button("Choose a folder…") {
                    shell.settingsTarget = SettingsDestination(
                        section: .models, anchor: "models.folders")
                    SettingsWindowController.shared.show(shell: shell)
                }
                .buttonStyle(QuietButtonStyle())
            }
        }
    }

    private var visible: [Suggestion] {
        Suggestion.catalog.filter { $0.category == category }
    }

    private var recommendedID: Suggestion.ID? {
        let fitting = visible.filter {
            let verdict = hardware.fit($0.sizeGB)
            return verdict == .easy || verdict == .good
        }
        return (fitting.max { $0.sizeGB < $1.sizeGB } ?? visible.min { $0.sizeGB < $1.sizeGB })?.id
    }

    private var suggestions: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            HStack(alignment: .center) {
                MicroHeader(title: "Recommended for your Mac")
                Spacer(minLength: Design.Space.l)
                categoryTabs
            }
            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: 220), spacing: Design.Space.l)],
                spacing: Design.Space.l
            ) {
                ForEach(visible) { suggestion in
                    SuggestionCard(
                        suggestion: suggestion,
                        fit: hardware.fit(suggestion.sizeGB),
                        recommended: suggestion.id == recommendedID)
                }
            }
        }
    }

    private var categoryTabs: some View {
        HStack(spacing: Design.Space.xs) {
            ForEach(SuggestionCategory.allCases) { item in
                let selected = item == category
                Button {
                    category = item
                } label: {
                    Text(item.rawValue)
                        .font(Design.micro)
                        .tracking(0.4)
                        .foregroundStyle(selected ? Design.onAccent : Design.inkSoft)
                        .padding(.horizontal, Design.Space.chipX)
                        .padding(.vertical, Design.Space.xs + 1)
                        .background(
                            selected ? AnyShapeStyle(Design.accent) : AnyShapeStyle(Design.panel),
                            in: RoundedRectangle.soft(Design.Radius.control))
                        .overlay(
                            RoundedRectangle.soft(Design.Radius.control)
                                .strokeBorder(
                                    selected ? Color.clear : Design.line,
                                    lineWidth: Design.hairlineWidth))
                        .contentShape(RoundedRectangle.soft(Design.Radius.control))
                }
                .buttonStyle(PressDipStyle())
                .accessibilityLabel(item.rawValue)
                .accessibilityAddTraits(selected ? .isSelected : [])
            }
        }
    }
}

struct SuggestionCard: View {
    let suggestion: Suggestion
    let fit: SuggestionFit
    let recommended: Bool
    @State private var hovering = false
    @State private var acted = false

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack {
                Text(suggestion.source.badge)
                    .font(Design.micro)
                    .foregroundStyle(Design.accentText)
                    .frame(width: 26, height: 24)
                    .overlay(
                        RoundedRectangle.soft(Design.Radius.control)
                            .strokeBorder(Design.lineBright, lineWidth: Design.hairlineWidth))
                Spacer(minLength: 0)
                Text((recommended ? "best for you" : fit.label).uppercased())
                    .font(Design.label)
                    .tracking(Design.microTracking)
                    .foregroundStyle(fit == .tight ? Design.heatText : Design.accentText)
            }
            Text(suggestion.name)
                .font(Design.title)
                .foregroundStyle(Design.ink)
                .lineLimit(1)
            Text(suggestion.blurb)
                .font(Design.readingBody)
                .lineSpacing(2)
                .foregroundStyle(Design.inkSoft)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            HStack {
                Text(String(format: "%g GB", suggestion.sizeGB))
                    .font(Design.data(11))
                    .foregroundStyle(Design.inkFaint)
                Spacer(minLength: Design.Space.m)
                Button(action: get) {
                    Text(acted ? "Copied ✓" : suggestion.source == .ollama ? "Get ▸" : "Open ▸")
                        .font(Design.micro)
                        .tracking(0.4)
                        .foregroundStyle(recommended ? Design.onAccent : Design.ink)
                        .padding(.horizontal, Design.Space.l)
                        .padding(.vertical, Design.Space.s)
                        .background(
                            recommended ? AnyShapeStyle(Design.accent) : AnyShapeStyle(.clear),
                            in: RoundedRectangle.soft(Design.Radius.control))
                        .overlay(
                            RoundedRectangle.soft(Design.Radius.control)
                                .strokeBorder(
                                    recommended ? Color.clear : Design.lineBright,
                                    lineWidth: Design.hairlineWidth))
                        .contentShape(RoundedRectangle.soft(Design.Radius.control))
                }
                .buttonStyle(PressDipStyle())
            }
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, minHeight: 150, alignment: .topLeading)
        .background(Design.surface, in: RoundedRectangle.soft(Design.Radius.card))
        .overlay(
            RoundedRectangle.soft(Design.Radius.card)
                .strokeBorder(
                    hovering
                        ? Design.accentEdge
                        : recommended ? Design.accentEdge : Design.line,
                    lineWidth: Design.hairlineWidth))
        .onHover { hovering = $0 }
        .animation(Design.wash, value: hovering)
        .help(suggestion.source == .ollama ? "Copy: ollama pull \(suggestion.name)" : suggestion.reference)
    }

    private func get() {
        switch suggestion.source {
        case .ollama:
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString("ollama pull \(suggestion.name)", forType: .string)
            acted = true
            Task {
                try? await Task.sleep(for: .seconds(1.6))
                acted = false
            }
        case .huggingface:
            if let url = URL(string: suggestion.reference) {
                NSWorkspace.shared.open(url)
            }
        }
    }
}
