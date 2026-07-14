import HedosKernel
import SwiftUI

struct SlashCommand: Identifiable {
    let id: String
    let title: String
    let subtitle: String
    let glyph: String
    let perform: () -> Void
}

struct SlashSetup {
    let kernel: Kernel
    let capability: Capability
    var commands: [SlashCommand] = []
}

struct SlashEntry: Identifiable {
    enum Kind {
        case command(SlashCommand)
        case prompt(Prompt)
        case file(String)
    }

    let kind: Kind

    var id: String {
        switch kind {
        case .command(let command): "command-\(command.id)"
        case .prompt(let prompt): "prompt-\(prompt.id)"
        case .file(let path): "file-\(path)"
        }
    }

    var glyph: String {
        switch kind {
        case .command(let command): command.glyph
        case .prompt: "text.quote"
        case .file: "doc.text"
        }
    }

    var title: String {
        switch kind {
        case .command(let command): "/" + command.title
        case .prompt(let prompt): prompt.title
        case .file(let path): path
        }
    }

    var subtitle: String {
        switch kind {
        case .command(let command):
            return command.subtitle
        case .prompt(let prompt):
            return prompt.placeholderNames.isEmpty
                ? "Prompt" : "Prompt · {\(prompt.placeholderNames.joined(separator: "} {"))}"
        case .file:
            return ""
        }
    }

    var matchTitle: String {
        switch kind {
        case .command(let command): command.title
        case .prompt(let prompt): prompt.title
        case .file(let path): path
        }
    }

    var isFile: Bool {
        if case .file = kind { return true }
        return false
    }
}

enum SlashMenu {
    static let capacity = 8

    static func entries(
        query: String, prompts: [Prompt], commands: [SlashCommand], capability: Capability
    ) -> [SlashEntry] {
        let candidates =
            commands.map { SlashEntry(kind: .command($0)) }
            + prompts
            .filter { $0.capability == nil || $0.capability == capability }
            .map { SlashEntry(kind: .prompt($0)) }
        return
            candidates
            .compactMap { entry -> (SlashEntry, Int)? in
                guard let score = PromptComposer.matchScore(query, against: entry.matchTitle)
                else { return nil }
                return (entry, score)
            }
            .sorted {
                ($0.1, $0.0.matchTitle.localizedLowercase)
                    < ($1.1, $1.0.matchTitle.localizedLowercase)
            }
            .prefix(capacity)
            .map(\.0)
    }
}

struct SlashMenuPanel: View {
    let entries: [SlashEntry]
    let highlighted: Int
    let onAccept: (SlashEntry) -> Void
    let onHighlight: (Int) -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private var allFiles: Bool {
        !entries.isEmpty && entries.allSatisfy(\.isFile)
    }

    private var rowHeight: CGFloat {
        allFiles ? 30 : 40
    }

    private var listHeight: CGFloat {
        CGFloat(min(entries.count, 6)) * rowHeight + Design.Space.s * 2
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Rectangle()
                .fill(Design.line)
                .frame(height: Design.hairlineWidth)
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: Design.Space.xxs) {
                        ForEach(Array(entries.enumerated()), id: \.element.id) { index, entry in
                            row(entry, index: index).id(index)
                        }
                    }
                    .padding(Design.Space.s)
                }
                .frame(height: listHeight)
                .scrollDisabled(entries.count <= 6)
                .onChange(of: highlighted) { _, new in
                    withAnimation(reduceMotion ? nil : Design.snap) {
                        proxy.scrollTo(new, anchor: .center)
                    }
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityIdentifier("slash-menu")
    }

    private var header: some View {
        Text(verbatim: "↑↓ navigate · ↵ select · esc dismiss")
            .font(Design.label)
            .foregroundStyle(Design.inkFaint)
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.s)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func row(_ entry: SlashEntry, index: Int) -> some View {
        Button {
            onAccept(entry)
        } label: {
            HStack(spacing: Design.Space.m) {
                Image(systemName: entry.glyph)
                    .font(Design.glyphInline)
                    .foregroundStyle(index == highlighted ? Design.ink : Design.inkSoft)
                    .frame(width: 16, alignment: .leading)
                if entry.isFile {
                    Text(entry.title)
                        .font(Design.caption)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .foregroundStyle(Design.ink)
                } else {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(entry.title)
                            .font(Design.caption.weight(.medium))
                            .lineLimit(1)
                            .foregroundStyle(Design.ink)
                        if !entry.subtitle.isEmpty {
                            Text(entry.subtitle)
                                .font(Design.label)
                                .lineLimit(1)
                                .truncationMode(.middle)
                                .foregroundStyle(Design.inkSoft)
                        }
                    }
                }
                Spacer(minLength: Design.Space.s)
            }
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, entry.isFile ? Design.Space.xs : Design.Space.s)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                index == highlighted ? Design.inkWash : .clear,
                in: RoundedRectangle.soft(Design.Radius.control))
            .contentShape(RoundedRectangle.soft(Design.Radius.control))
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            if hovering {
                onHighlight(index)
            }
        }
        .accessibilityLabel(entry.title)
        .accessibilityAddTraits(index == highlighted ? .isSelected : [])
    }
}
