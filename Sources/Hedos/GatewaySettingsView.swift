import AppKit
import HedosKernel
import SwiftUI

struct GatewaySection: View {
    @Bindable var shell: ShellModel
    let highlighted: String?
    let onAddClient: () -> Void
    let onConnect: () -> Void
    var onShowAllRequests: (() -> Void)? = nil
    var showsControlHeader = true
    @State private var portText = ""
    @State private var portDenyCount = 0
    @State private var portError = false
    @State private var portRevert: Task<Void, Never>?
    @State private var hoveredClient: String?

    private var model: SettingsModel { shell.settings }

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.xxl) {
            if showsControlHeader {
                controlHeader
            } else if model.gatewayStatus.running {
                addressCard
            }
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
                RowRule()
                SettingRow(
                    id: "gateway.port", label: "Port",
                    caption: "Loopback only, always. Press Return to apply.",
                    highlighted: highlighted == "gateway.port"
                ) {
                    VStack(alignment: .leading, spacing: Design.Space.xxs) {
                        InkField(placeholder: "43367", text: $portText, size: .settings)
                            .frame(width: Design.Control.fieldWidthNarrow)
                            .onSubmit { applyPort() }
                            .denyShake(
                                on: portDenyCount,
                                in: RoundedRectangle.soft(Design.Radius.control))
                            .accessibilityIdentifier("gateway-port")
                        if portError {
                            Text("1024–65535")
                                .font(Design.label)
                                .foregroundStyle(Design.heatText)
                                .transition(.arrive(from: .top))
                        }
                    }
                    .animation(Design.wash, value: portError)
                    .onChange(of: portText) {
                        if portError {
                            portError = false
                            portRevert?.cancel()
                        }
                    }
                }
            }
            group("Endpoints") {
                endpointRows
            }
            group("Clients") {
                clientRows
            }
            group("Recent requests") {
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
                            .frame(width: 56, alignment: .leading)
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
            Button("Connect a tool…") {
                onConnect()
            }
            .buttonStyle(QuietButtonStyle())
            .accessibilityIdentifier("gateway-connect")
            .padding(.top, Design.Space.xs)
        }
        .padding(.vertical, Design.Space.m)
        .id("gateway.endpoints")
        .background(highlightBackground("gateway.endpoints"))
    }

    private var controlHeader: some View {
        let running = model.gatewayStatus.running
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: Design.Space.chipX) {
                if running {
                    AccentDot(size: 9)
                } else {
                    Circle()
                        .fill(Design.inkFaint)
                        .frame(width: 9, height: 9)
                }
                Text("Gateway")
                    .font(Design.title)
                    .foregroundStyle(Design.ink)
                Spacer(minLength: Design.Space.m)
                Text(running ? "LIVE · :\(String(port))" : "OFFLINE")
                    .font(Design.micro)
                    .tracking(Design.microTracking)
                    .foregroundStyle(running ? Design.accentText : Design.inkFaint)
            }
            .padding(.horizontal, Design.Space.tile)
            .padding(.vertical, Design.Space.l)
            if running {
                Rectangle()
                    .fill(Design.line)
                    .frame(height: Design.hairlineWidth)
                addressStrip
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(radius: Design.Radius.tile)
        .accessibilityIdentifier("gateway-address")
    }

    private var addressCard: some View {
        addressStrip
            .frame(maxWidth: .infinity, alignment: .leading)
            .surfaceCard(radius: Design.Radius.tile)
            .accessibilityIdentifier("gateway-address")
    }

    private var addressStrip: some View {
        HStack(spacing: Design.Space.s) {
            Text(verbatim: "$")
                .font(Design.data(12))
                .foregroundStyle(Design.inkFaint)
            Text(address)
                .font(Design.data(12))
                .foregroundStyle(Design.ink)
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: Design.Space.m)
            ConfirmingButton(
                label: "COPY", confirmedLabel: "COPIED", appearance: .micro
            ) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(address, forType: .string)
            }
            .accessibilityLabel("Copy gateway address")
        }
        .padding(.horizontal, Design.Space.tile)
        .padding(.vertical, Design.Space.l)
    }

    private var port: Int {
        model.gatewayStatus.port ?? model.gateway.port
    }

    private var address: String {
        GatewayDefaults.baseURL(port: port)
    }

    private func lastUsedShort(_ client: GatewayClient) -> String {
        guard let used = client.lastUsedAt else { return "never" }
        return Self.relative.localizedString(for: used, relativeTo: Date())
    }

    private var clientRows: some View {
        let auditCounts = Dictionary(grouping: model.gatewayAuditEntries, by: \.client)
            .mapValues(\.count)
        return VStack(alignment: .leading, spacing: 0) {
            if !model.gatewayClients.isEmpty {
                HStack(spacing: Design.Space.m) {
                    Text("CLIENT")
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Text("RECENT")
                        .frame(width: 60, alignment: .trailing)
                    Text("LAST USED")
                        .frame(width: 116, alignment: .trailing)
                    Color.clear.frame(width: 18, height: 1)
                }
                .font(Design.label)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkFaint)
                .padding(.vertical, Design.Space.s)
            }
            ForEach(model.gatewayClients) { client in
                clientLedgerRow(client, recent: auditCounts[client.id] ?? 0)
            }
            if model.gatewayClients.isEmpty {
                Text("Every request needs a token, loopback included. Create one per tool.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.vertical, Design.Space.s)
            }
            Button("Add a client…") {
                onAddClient()
            }
            .buttonStyle(QuietButtonStyle())
            .accessibilityIdentifier("gateway-add-client")
            .padding(.top, Design.Space.m)
        }
        .padding(.vertical, Design.Space.m)
        .id("gateway.clients")
        .background(highlightBackground("gateway.clients"))
    }

    private func clientLedgerRow(_ client: GatewayClient, recent: Int) -> some View {
        HStack(spacing: Design.Space.m) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: Design.Space.s) {
                    if client.lastUsedAt != nil {
                        AccentDot(size: 7)
                    } else {
                        Circle()
                            .fill(Design.inkFaint)
                            .frame(width: 7, height: 7)
                    }
                    Text(client.name)
                        .font(Design.body.weight(.medium))
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                }
                Text(scopeSummary(client.scopes))
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Text("\(recent)")
                .font(Design.data(12))
                .monospacedDigit()
                .foregroundStyle(recent > 0 ? Design.ink : Design.inkFaint)
                .frame(width: 60, alignment: .trailing)
            Text(lastUsedShort(client))
                .font(Design.data(11))
                .foregroundStyle(Design.inkFaint)
                .lineLimit(1)
                .frame(width: 116, alignment: .trailing)
            ConfirmableIconButton(
                label: "Revoke \(client.name)", confirmLabel: "Revoke?"
            ) {
                model.revokeGatewayClient(id: client.id)
            }
            .fixedSize()
        }
        .padding(.vertical, Design.Space.s)
        .padding(.horizontal, Design.Space.s)
        .background(
            RoundedRectangle.soft(Design.Radius.control)
                .fill(hoveredClient == client.id ? Design.inkWash : .clear))
        .padding(.horizontal, -Design.Space.s)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Design.line)
                .frame(height: Design.hairlineWidth)
        }
        .onHover { inside in
            if inside {
                hoveredClient = client.id
            } else if hoveredClient == client.id {
                hoveredClient = nil
            }
        }
        .animation(Design.wash, value: hoveredClient == client.id)
    }

    private var auditRows: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(model.gatewayAuditEntries.prefix(12).enumerated()), id: \.offset) {
                _, entry in
                HStack(spacing: Design.Space.m) {
                    Text(Self.time.string(from: entry.ts))
                        .font(Design.data(11))
                        .monospacedDigit()
                        .foregroundStyle(Design.inkFaint)
                    Text(entry.method)
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkSoft)
                        .frame(width: 46, alignment: .leading)
                    Text(entry.route)
                        .font(Design.data(11))
                        .foregroundStyle(Design.ink)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Text(verbatim: "→")
                        .font(Design.data(11))
                        .foregroundStyle(Design.inkFaint)
                    Text(String(entry.status))
                        .font(Design.data(11))
                        .monospacedDigit()
                        .foregroundStyle(entry.outcome == "ok" ? Design.inkSoft : Design.danger)
                    Text("\(entry.durationMs)ms")
                        .font(Design.data(10))
                        .monospacedDigit()
                        .foregroundStyle(Design.inkFaint)
                        .frame(width: 58, alignment: .trailing)
                }
                .padding(.vertical, Design.Space.s)
                .overlay(alignment: .top) {
                    Rectangle()
                        .fill(Design.line)
                        .frame(height: Design.hairlineWidth)
                }
            }
            if model.gatewayAuditEntries.isEmpty {
                Text("Requests land here — successes and refusals both.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.vertical, Design.Space.s)
            }
            HStack(spacing: Design.Space.m) {
                if model.gatewayAuditEntries.count > 12, let onShowAllRequests {
                    Button("Show all \(model.gatewayAuditEntries.count)") {
                        onShowAllRequests()
                    }
                    .buttonStyle(QuietButtonStyle())
                }
                Button("Show in Finder") {
                    NSWorkspace.shared.activateFileViewerSelecting(
                        [model.gatewayAuditFileURL])
                }
                .buttonStyle(QuietButtonStyle())
            }
            .padding(.top, Design.Space.m)
        }
        .padding(.vertical, Design.Space.m)
        .id("gateway.audit")
        .background(highlightBackground("gateway.audit"))
    }

    private func applyPort() {
        guard let port = Int(portText), GatewayDefaults.portRange.contains(port) else {
            portDenyCount += 1
            portError = true
            portRevert?.cancel()
            portRevert = Task {
                try? await Task.sleep(for: .milliseconds(600))
                guard !Task.isCancelled else { return }
                portText = String(model.gateway.port)
                portError = false
            }
            return
        }
        portRevert?.cancel()
        portError = false
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
        _ header: String, @ViewBuilder content: @escaping () -> some View
    ) -> some View {
        SettingsGroup(header: header, content: content)
    }

    private func highlightBackground(_ id: String) -> some View {
        RoundedRectangle.soft(Design.Radius.card)
            .fill(highlighted == id ? Design.ink.opacity(0.08) : .clear)
            .padding(.horizontal, -Design.Space.s)
    }
}

struct GatewayCodeBlock: View {
    let title: String
    let code: String
    var language: String? = nil
    @State private var copied = false
    @State private var tokens: [CodeToken] = []

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
            highlighted
                .font(Design.data(11))
                .lineSpacing(2.5)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
                .padding(Design.Space.tile)
                .background(
                    RoundedRectangle.soft(Design.Radius.tile)
                        .fill(Design.surface))
                .overlay(
                    RoundedRectangle.soft(Design.Radius.tile)
                        .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        }
        .onAppear { tokens = CodeHighlighter.tokens(code, language: resolvedLanguage) }
        .onChange(of: code) { _, newCode in
            tokens = CodeHighlighter.tokens(newCode, language: resolvedLanguage)
        }
    }

    private var resolvedLanguage: String? {
        if let language { return language }
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("{") || trimmed.hasPrefix("[") { return "json" }
        if trimmed.contains("import ") || trimmed.contains("from ") || trimmed.contains("def ") {
            return "python"
        }
        if trimmed.contains("curl") || trimmed.hasPrefix("$") { return "bash" }
        return nil
    }

    private var highlighted: Text {
        tokens.reduce(Text(verbatim: "")) { result, token in
            Text("\(result)\(styled(token))")
        }
    }

    private func styled(_ token: CodeToken) -> Text {
        switch token.kind {
        case .plain:
            Text(verbatim: token.text).foregroundStyle(Design.ink)
        case .keyword:
            Text(verbatim: token.text).foregroundStyle(Design.ink).fontWeight(.semibold)
        case .string:
            Text(verbatim: token.text).foregroundStyle(Design.inkSoft)
        case .comment:
            Text(verbatim: token.text).foregroundStyle(Design.inkFaint)
        case .number:
            Text(verbatim: token.text).foregroundStyle(Design.accentText)
        }
    }
}
