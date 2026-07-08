import AppKit
import HedosKernel
import SwiftUI

struct GatewaySection: View {
    @Bindable var shell: ShellModel
    let highlighted: String?
    let onAddClient: () -> Void
    let onConnect: () -> Void
    @State private var portText = ""
    @State private var copiedAddress = false

    private var model: SettingsModel { shell.settings }

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.xxl) {
            group("Serve") {
                SettingRow(
                    id: "gateway.enable", label: "Serve models over HTTP",
                    caption:
                        "OpenAI and Ollama dialects on one local port. Off means no socket exists.",
                    highlighted: highlighted == "gateway.enable"
                ) {
                    InkToggle(
                        isOn: model.gateway.enabled, isSet: true,
                        onToggle: { model.setGatewayEnabled($0) })
                }
                .accessibilityIdentifier("gateway-enable")
                if model.gatewayStatus.running, let port = model.gatewayStatus.port {
                    addressRow(port: port)
                }
                if let notice = model.gatewayNotice {
                    HStack(spacing: Design.Space.s) {
                        Image(systemName: "exclamationmark.triangle")
                            .font(Design.glyphInline)
                            .foregroundStyle(Design.inkSoft)
                        Text(notice)
                            .font(Design.caption.weight(.medium))
                            .foregroundStyle(Design.inkSoft)
                    }
                    .padding(.vertical, Design.Space.s)
                }
                Divider()
                SettingRow(
                    id: "gateway.port", label: "Port",
                    caption: "Loopback only, always. Press Return to apply.",
                    highlighted: highlighted == "gateway.port"
                ) {
                    InkField(placeholder: "43367", text: $portText)
                        .frame(width: 96)
                        .onSubmit { applyPort() }
                        .accessibilityIdentifier("gateway-port")
                }
            }
            group("Endpoints") {
                endpointRows
            }
            group("Client tokens") {
                clientRows
            }
            group("Recent activity") {
                auditRows
            }
        }
        .task {
            await model.refreshGateway()
            portText = String(model.gateway.port)
        }
    }

    private var endpointRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack {
                Spacer()
                Button("Connect a tool…") {
                    onConnect()
                }
                .buttonStyle(QuietButtonStyle())
                .accessibilityIdentifier("gateway-connect")
            }
            ForEach(Array(GatewayEndpoints.grouped.enumerated()), id: \.offset) { _, section in
                Text(section.group.uppercased())
                    .font(Design.micro)
                    .tracking(Design.microTracking)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.top, Design.Space.xs)
                ForEach(section.endpoints) { endpoint in
                    HStack(spacing: Design.Space.s) {
                        Text(endpoint.method)
                            .font(Design.micro)
                            .tracking(Design.microTracking)
                            .foregroundStyle(Design.inkSoft)
                            .frame(width: 40, alignment: .leading)
                        Text(endpoint.path)
                            .font(Design.data(12))
                            .foregroundStyle(Design.ink)
                            .textSelection(.enabled)
                        Spacer(minLength: Design.Space.m)
                        Text(endpoint.summary)
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                            .lineLimit(1)
                            .truncationMode(.tail)
                    }
                }
            }
            if !model.gatewayStatus.running {
                Text("Turn serving on above to reach these.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.top, Design.Space.xs)
            }
        }
        .padding(.vertical, Design.Space.m)
        .id("gateway.endpoints")
        .background(highlightBackground("gateway.endpoints"))
    }

    private func addressRow(port: Int) -> some View {
        HStack(spacing: Design.Space.s) {
            AccentDot()
            Text("http://127.0.0.1:\(String(port))/v1")
                .font(Design.data(12))
                .foregroundStyle(Design.ink)
                .textSelection(.enabled)
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(
                    "http://127.0.0.1:\(String(port))/v1", forType: .string)
                copiedAddress = true
                Task {
                    try? await Task.sleep(for: .seconds(2))
                    copiedAddress = false
                }
            } label: {
                Image(systemName: copiedAddress ? "checkmark" : "doc.on.doc")
                    .font(Design.glyphInline)
                    .foregroundStyle(Design.inkSoft)
            }
            .buttonStyle(PressDipStyle())
            .accessibilityLabel("Copy gateway address")
            Spacer()
        }
        .padding(.vertical, Design.Space.s)
        .accessibilityIdentifier("gateway-address")
    }

    private var clientRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack {
                Spacer()
                Button("Add a client…") {
                    onAddClient()
                }
                .buttonStyle(QuietButtonStyle())
                .accessibilityIdentifier("gateway-add-client")
            }
            ForEach(model.gatewayClients) { client in
                HStack(spacing: Design.Space.s) {
                    Image(systemName: "key")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkSoft)
                    Text(client.name)
                        .font(Design.label)
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                    Text(scopeSummary(client.scopes))
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    Text(usedLabel(client))
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                    Button {
                        model.revokeGatewayClient(id: client.id)
                    } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(Design.glyphInline)
                            .foregroundStyle(Design.inkFaint)
                    }
                    .buttonStyle(PressDipStyle())
                    .accessibilityLabel("Revoke \(client.name)")
                }
            }
            if model.gatewayClients.isEmpty {
                Text("Every request needs a token, loopback included. Create one per tool.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
        }
        .padding(.vertical, Design.Space.m)
        .id("gateway.clients")
        .background(highlightBackground("gateway.clients"))
    }

    private var auditRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            HStack {
                Spacer()
                Button("Show in Finder") {
                    NSWorkspace.shared.activateFileViewerSelecting(
                        [model.gatewayAuditFileURL])
                }
                .buttonStyle(QuietButtonStyle())
            }
            ForEach(Array(model.gatewayAuditEntries.enumerated()), id: \.offset) { _, entry in
                HStack(spacing: Design.Space.s) {
                    Text(Self.time.string(from: entry.ts))
                        .font(Design.data(11))
                        .foregroundStyle(Design.inkFaint)
                    Text(entry.outcome)
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(entry.outcome == "ok" ? Design.inkSoft : Design.ink)
                    Text("\(entry.method) \(entry.route)")
                        .font(Design.data(11))
                        .foregroundStyle(Design.inkSoft)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer()
                    if let name = entry.clientName {
                        Text(name)
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                            .lineLimit(1)
                    }
                    Text(String(entry.status))
                        .font(Design.data(11))
                        .foregroundStyle(Design.inkFaint)
                }
            }
            if model.gatewayAuditEntries.isEmpty {
                Text("Requests land here — successes and refusals both.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
        }
        .padding(.vertical, Design.Space.m)
        .id("gateway.audit")
        .background(highlightBackground("gateway.audit"))
    }

    private func applyPort() {
        guard let port = Int(portText), (1024...65535).contains(port) else {
            portText = String(model.gateway.port)
            return
        }
        model.gateway.port = port
        model.applyGatewayPort()
    }

    private func scopeSummary(_ scopes: GatewayScopes) -> String {
        let models: String
        if let ids = scopes.models {
            let names = ids.compactMap { shell.library.record(id: $0)?.displayName }
            models = names.isEmpty ? "\(ids.count) models" : names.joined(separator: ", ")
        } else {
            models = "all models"
        }
        let capabilities = scopes.capabilities?.joined(separator: ", ") ?? "all capabilities"
        return "\(models) · \(capabilities)"
    }

    private func usedLabel(_ client: GatewayClient) -> String {
        guard let used = client.lastUsedAt else { return "NEVER USED" }
        return "USED " + Self.relative.localizedString(for: used, relativeTo: Date()).uppercased()
    }

    private static let time: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    private static let relative = RelativeDateTimeFormatter()

    private func group(
        _ header: String, @ViewBuilder content: () -> some View
    ) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: header)
            VStack(alignment: .leading, spacing: 0) {
                content()
            }
            .padding(.horizontal, Design.Space.tile)
            .padding(.vertical, Design.Space.xs)
            .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private func highlightBackground(_ id: String) -> some View {
        RoundedRectangle(cornerRadius: Design.Radius.card)
            .fill(highlighted == id ? Design.ink.opacity(0.08) : .clear)
            .padding(.horizontal, -Design.Space.s)
    }
}

struct GatewayCodeBlock: View {
    let title: String
    let code: String
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.xs) {
            HStack(spacing: Design.Space.s) {
                Text(title.uppercased())
                    .font(Design.micro)
                    .tracking(Design.microTracking)
                    .foregroundStyle(Design.inkFaint)
                Spacer()
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(code, forType: .string)
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(2))
                        copied = false
                    }
                } label: {
                    Image(systemName: copied ? "checkmark" : "doc.on.doc")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkSoft)
                }
                .buttonStyle(PressDipStyle())
                .accessibilityLabel("Copy \(title)")
            }
            Text(code)
                .font(Design.data(11))
                .foregroundStyle(Design.ink)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
                .padding(Design.Space.tile)
                .background(
                    RoundedRectangle(cornerRadius: Design.Radius.tile)
                        .fill(Design.surface))
                .overlay(
                    RoundedRectangle(cornerRadius: Design.Radius.tile)
                        .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        }
    }
}
