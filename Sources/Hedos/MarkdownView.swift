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
                .font(Design.body)
                .lineSpacing(Design.bodyLineSpacing)
                .textSelection(.enabled)
        case .heading(let level, let text):
            inline(text)
                .font(Design.markdownHeading(level))
                .textSelection(.enabled)
                .padding(.top, Design.Space.xs)
        case .code(let language, let code, _):
            CodeBlockView(language: language, code: code)
        case .list(let items, let ordered):
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                    HStack(alignment: .firstTextBaseline, spacing: 7) {
                        Text(ordered ? "\(index + 1)." : "•")
                            .font(Design.body)
                            .foregroundStyle(Design.inkFaint)
                        inline(item)
                            .font(Design.body)
                            .lineSpacing(Design.bodyLineSpacing)
                            .textSelection(.enabled)
                    }
                }
            }
        case .quote(let text):
            inline(text)
                .font(Design.body)
                .lineSpacing(Design.bodyLineSpacing)
                .foregroundStyle(Design.inkSoft)
                .textSelection(.enabled)
                .leftRule()
        case .table(let header, let rows):
            ScrollView(.horizontal) {
                Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 6) {
                    GridRow {
                        ForEach(Array(header.enumerated()), id: \.offset) { _, cell in
                            inline(cell)
                                .font(Design.caption.weight(.semibold))
                        }
                    }
                    Divider()
                    ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                        GridRow {
                            ForEach(Array(row.enumerated()), id: \.offset) { _, cell in
                                inline(cell)
                                    .font(Design.caption)
                                    .textSelection(.enabled)
                            }
                        }
                    }
                }
                .padding(Design.Space.l)
            }
            .background(Design.tableFill, in: RoundedRectangle(cornerRadius: Design.Radius.card))
        case .rule:
            Divider()
                .padding(.vertical, Design.Space.xxs)
        }
    }

    private func inline(_ text: String) -> Text {
        Text(Self.attributed(text))
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
                    .foregroundStyle(Design.inkFaint)
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
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkFaint)
                }
                .buttonStyle(.plain)
                .help("Copy code")
                .accessibilityLabel("Copy code")
                .opacity(hovering || copied ? 1 : 0)
            }
            .padding(.horizontal, Design.Space.l)
            .padding(.top, Design.Space.m)
            .padding(.bottom, Design.Space.s)
            ScrollView(.horizontal) {
                highlighted
                    .font(Design.data(12))
                    .lineSpacing(2.5)
                    .textSelection(.enabled)
                    .padding(.horizontal, Design.Space.l)
                    .padding(.bottom, Design.Space.l)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Design.cardFill, in: RoundedRectangle(cornerRadius: Design.Radius.card))
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
            Text(verbatim: token.text).foregroundStyle(Design.inkSoft)
        case .comment:
            Text(verbatim: token.text).foregroundStyle(Design.inkFaint)
        case .number:
            Text(verbatim: token.text).foregroundStyle(Design.inkSoft)
        }
    }
}
