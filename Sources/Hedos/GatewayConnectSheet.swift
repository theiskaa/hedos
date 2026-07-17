import AppKit
import HedosKernel
import SwiftUI

struct GatewayConnectSheet: View {
    let shell: ShellModel
    let onClose: () -> Void

    @State private var sampleModel: String?
    @State private var copiedBase = false

    private var port: Int {
        shell.settings.gatewayStatus.port ?? shell.settings.gateway.port
    }

    private var readyModels: [ModelRecord] {
        shell.library.records.filter { $0.state == .ready }
    }

    private var chatModels: [ModelRecord] {
        readyModels.filter { $0.capabilities.contains(.chat) }
    }

    private var model: String {
        sampleModel ?? chatModels.first?.displayName ?? readyModels.first?.displayName
            ?? "your-model"
    }

    private var token: String { GatewayExamples.tokenPlaceholder }

    private var speakModel: ModelRecord? {
        readyModels.first { $0.capabilities.contains(.speak) }
    }

    private var imageModel: ModelRecord? {
        readyModels.first { $0.capabilities.contains(.image) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.gutter)
                .padding(.bottom, Design.Space.xl)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    baseSection
                    curlSection
                    clientSection
                    if speakModel != nil || imageModel != nil {
                        moreSection
                    }
                    caveats
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.xl)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            footer
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.l)
        }
        .frame(width: 680, height: 640)
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                Image(systemName: "cable.connector")
                    .font(Design.glyphNav)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text("Connect a tool")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                Text("Point any OpenAI- or Ollama-compatible tool here")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Spacer()
            SheetCloseButton(action: onClose)
        }
    }

    private var baseSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Base URL")
            HStack(spacing: Design.Space.s) {
                Text(GatewayExamples.baseURL(port: port))
                    .font(Design.data(12))
                    .foregroundStyle(Design.ink)
                    .textSelection(.enabled)
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(
                        GatewayExamples.baseURL(port: port), forType: .string)
                    copiedBase = true
                    Task {
                        try? await Task.sleep(for: .seconds(2))
                        copiedBase = false
                    }
                } label: {
                    Image(systemName: copiedBase ? "checkmark" : "doc.on.doc")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkSoft)
                }
                .buttonStyle(PressDipStyle())
                .accessibilityLabel("Copy base URL")
            }
            .padding(Design.Space.tile)
            .surfaceCard(radius: Design.Radius.tile)
            if !chatModels.isEmpty {
                HStack(spacing: Design.Space.m) {
                    Text("Examples use")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                    InkDropdown(
                        options: chatModels.map(\.displayName),
                        selection: model,
                        allowsAuto: false,
                        accessibilityName: "example model"
                    ) { picked in
                        sampleModel = picked
                    }
                }
            }
        }
    }

    private var curlSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "curl")
            GatewayCodeBlock(
                title: "chat completion",
                code: GatewayExamples.chatCurl(port: port, model: model, token: token))
            GatewayCodeBlock(
                title: "list models",
                code: GatewayExamples.modelsCurl(port: port, token: token))
        }
    }

    private var clientSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Clients")
            GatewayCodeBlock(
                title: "OpenAI Python SDK",
                code: GatewayExamples.openAISDK(port: port, model: model, token: token))
            GatewayCodeBlock(
                title: "ollama Python client",
                code: GatewayExamples.ollamaClient(port: port, model: model, token: token))
        }
    }

    private var moreSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "More")
            if let speak = speakModel {
                GatewayCodeBlock(
                    title: "speak → wav",
                    code: GatewayExamples.speechCurl(
                        port: port, model: speak.displayName, token: token))
            }
            if let image = imageModel {
                GatewayCodeBlock(
                    title: "generate an image",
                    code: GatewayExamples.imagesCurl(
                        port: port, model: image.displayName, token: token))
            }
        }
    }

    private var caveats: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            caveat("Replace \(GatewayExamples.tokenPlaceholder) with a client token — create one in Client tokens.")
            caveat("Every request needs a token, even on loopback.")
            caveat("The raw ollama CLI can't send an auth header, so it gets 401 — use the client library or any tool that sets a bearer token.")
        }
    }

    private func caveat(_ text: String) -> some View {
        HStack(alignment: .top, spacing: Design.Space.s) {
            Image(systemName: "info.circle")
                .font(Design.glyphInline)
                .foregroundStyle(Design.inkFaint)
            Text(text)
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var footer: some View {
        HStack {
            Spacer()
            Button("Done") { onClose() }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
        }
    }
}
