import AppKit
import HedosKernel
import SwiftUI

struct MarkdownTurnView: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(
                Array(MarkdownBlocks.parse(text).enumerated()), id: \.offset
            ) { _, block in
                MarkdownBlockView(block: block)
            }
        }
    }
}

struct MarkdownBlockView: View {
    let block: MarkdownBlock

    var body: some View {
        switch block {
        case .paragraph(let text):
            inline(text)
                .font(.system(size: 13))
                .lineSpacing(3.5)
                .textSelection(.enabled)
        case .heading(let level, let text):
            inline(text)
                .font(.system(size: headingSize(level), weight: .semibold))
                .textSelection(.enabled)
                .padding(.top, 4)
        case .code(let language, let code, _):
            CodeBlockView(language: language, code: code)
        case .list(let items, let ordered):
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                    HStack(alignment: .firstTextBaseline, spacing: 7) {
                        Text(ordered ? "\(index + 1)." : "•")
                            .font(.system(size: 13))
                            .foregroundStyle(.tertiary)
                        inline(item)
                            .font(.system(size: 13))
                            .lineSpacing(3.5)
                            .textSelection(.enabled)
                    }
                }
            }
        case .quote(let text):
            inline(text)
                .font(.system(size: 13))
                .lineSpacing(3.5)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .padding(.leading, 10)
                .overlay(alignment: .leading) {
                    Rectangle()
                        .fill(.quaternary)
                        .frame(width: 2)
                }
        case .table(let header, let rows):
            ScrollView(.horizontal) {
                Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 6) {
                    GridRow {
                        ForEach(Array(header.enumerated()), id: \.offset) { _, cell in
                            inline(cell)
                                .font(.system(size: 12, weight: .semibold))
                        }
                    }
                    Divider()
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        GridRow {
                            ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                                inline(cell)
                                    .font(.system(size: 12))
                                    .textSelection(.enabled)
                            }
                        }
                    }
                }
                .padding(10)
            }
            .background(.quaternary.opacity(0.25), in: RoundedRectangle(cornerRadius: 8))
        case .rule:
            Divider()
                .padding(.vertical, 2)
        }
    }

    private func inline(_ text: String) -> Text {
        Text(Self.attributed(text))
    }

    private func headingSize(_ level: Int) -> CGFloat {
        switch level {
        case 1: 19
        case 2: 16.5
        case 3: 14.5
        default: 13.5
        }
    }

    static func attributed(_ text: String) -> AttributedString {
        (try? AttributedString(
            markdown: text,
            options: AttributedString.MarkdownParsingOptions(
                interpretedSyntax: .inlineOnlyPreservingWhitespace)))
            ?? AttributedString(text)
    }
}

struct CodeBlockView: View {
    let language: String?
    let code: String
    @State private var hovering = false
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language ?? "code")
                    .font(Design.data(10))
                    .foregroundStyle(.tertiary)
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(code, forType: .string)
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(1.5))
                        copied = false
                    }
                } label: {
                    Image(systemName: copied ? "checkmark" : "doc.on.doc")
                        .font(.system(size: 10))
                        .foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
                .help("Copy code")
                .opacity(hovering || copied ? 1 : 0)
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)
            .padding(.bottom, 6)
            ScrollView(.horizontal) {
                highlighted
                    .font(Design.data(12))
                    .lineSpacing(2.5)
                    .textSelection(.enabled)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 10)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 8))
        .onHover { hovering = $0 }
    }

    private var highlighted: Text {
        CodeHighlighter.tokens(code, language: language).reduce(Text(verbatim: "")) {
            result, token in
            result + styled(token)
        }
    }

    private func styled(_ token: CodeToken) -> Text {
        switch token.kind {
        case .plain:
            Text(verbatim: token.text)
        case .keyword:
            Text(verbatim: token.text).fontWeight(.semibold)
        case .string:
            Text(verbatim: token.text).foregroundStyle(.secondary)
        case .comment:
            Text(verbatim: token.text).foregroundStyle(.tertiary)
        case .number:
            Text(verbatim: token.text).foregroundStyle(.secondary)
        }
    }
}
