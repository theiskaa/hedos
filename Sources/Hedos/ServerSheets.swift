import HedosKernel
import SwiftUI

struct AddServerSheet: View {
    let shell: ShellModel
    let onClose: () -> Void

    @State private var baseURL = ""
    @State private var apiKey = ""
    @State private var models: [String] = []
    @State private var picked: String?
    @State private var added: [String] = []
    @State private var notice: String?
    @State private var connecting = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.gutter)
                .padding(.bottom, Design.Space.xl)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    connectSection
                    if !models.isEmpty {
                        pickSection
                    }
                    if let notice {
                        HStack(spacing: Design.Space.s) {
                            Image(systemName: "exclamationmark.triangle")
                                .font(Design.glyphInline)
                                .foregroundStyle(Design.inkSoft)
                            Text(notice)
                                .font(Design.caption.weight(.medium))
                                .foregroundStyle(Design.inkSoft)
                        }
                    }
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
        .frame(width: Design.Sheet.serverWidth)
        .frame(maxHeight: Design.Sheet.serverHeight)
    }

    private var header: some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            IconPlaque(size: 44) {
                Image(systemName: "network")
                    .font(Design.glyphNav)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text("Add a server")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                Text("Any OpenAI-compatible local server")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
            Spacer()
            SheetCloseButton(action: onClose)
        }
    }

    private var connectSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            InkField(placeholder: "http://127.0.0.1:11434", text: $baseURL)
            InkField(placeholder: "API key (optional)", text: $apiKey)
            HStack {
                Spacer()
                Button(connecting ? "Connecting…" : "Connect") {
                    connect()
                }
                .buttonStyle(QuietButtonStyle())
                .disabled(baseURL.trimmingCharacters(in: .whitespaces).isEmpty || connecting)
            }
        }
        .padding(Design.Space.tile)
        .surfaceCard(radius: Design.Radius.tile)
    }

    private var pickSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack(spacing: Design.Space.m) {
                InkDropdown(
                    options: models,
                    selection: picked,
                    placeholder: "pick a model",
                    allowsAuto: false,
                    accessibilityName: "server model"
                ) { picked = $0 }
                Button("Add this model") {
                    addPicked()
                }
                .buttonStyle(InkButtonStyle())
                .disabled(picked == nil)
            }
            ForEach(added, id: \.self) { model in
                HStack(spacing: Design.Space.s) {
                    Image(systemName: "checkmark.circle")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkSoft)
                    Text(model)
                        .font(Design.label)
                        .foregroundStyle(Design.ink)
                }
            }
        }
        .padding(Design.Space.tile)
        .surfaceCard(radius: Design.Radius.tile)
    }

    private var footer: some View {
        HStack {
            Spacer()
            Button("Done") {
                onClose()
            }
            .buttonStyle(InkButtonStyle())
            .keyboardShortcut(.defaultAction)
        }
    }

    private func connect() {
        connecting = true
        notice = nil
        let shell = shell
        let url = baseURL
        let key = apiKey.isEmpty ? nil : apiKey
        Task { @MainActor in
            let result = await shell.library.connectServer(baseURL: url, apiKey: key)
            connecting = false
            if let list = result.models {
                models = list
                picked = list.first
                if list.isEmpty {
                    notice = "The server answered but reported no models."
                }
            } else {
                models = []
                notice = result.error ?? "The server could not be reached."
            }
        }
    }

    private func addPicked() {
        guard let model = picked else { return }
        let shell = shell
        let url = baseURL
        Task { @MainActor in
            let ok = await shell.library.addEndpoint(baseURL: url, model: model)
            if ok {
                if !added.contains(model) { added.append(model) }
            } else {
                notice = "Couldn't add \(model)."
            }
        }
    }
}
