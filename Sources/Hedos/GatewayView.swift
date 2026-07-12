import AppKit
import HedosKernel
import SwiftUI

struct GatewayPane: View {
    @Bindable var shell: ShellModel
    @State private var showingAddClient = false
    @State private var showingConnect = false
    @State private var portText = ""
    @State private var copiedAddress = false

    private static let contentWidth: CGFloat = 1080

    private var model: SettingsModel { shell.settings }
    private var running: Bool { model.gatewayStatus.running }
    private var port: Int { model.gatewayStatus.port ?? model.gateway.port }
    private var address: String { "http://127.0.0.1:\(port)/v1" }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Design.Space.pane) {
                hero
                board
                clientsSection
                examplesSection
                endpointsSection
                auditSection
            }
            .padding(.horizontal, Design.Space.gutter)
            .padding(.top, Design.Space.xxl)
            .padding(.bottom, Design.Space.pane)
            .frame(maxWidth: Self.contentWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(PixelGrid())
        .task {
            await model.refreshGateway()
            portText = String(model.gateway.port)
        }
        .modalScrim(
            isPresented: showingAddClient,
            onDismiss: { showingAddClient = false }
        ) {
            AddGatewayClientSheet(shell: shell) { showingAddClient = false }
        }
        .modalScrim(
            isPresented: showingConnect,
            onDismiss: { showingConnect = false }
        ) {
            GatewayConnectSheet(shell: shell) { showingConnect = false }
        }
        .modalScrim(
            isPresented: shell.showingGatewayLog,
            onDismiss: { shell.showingGatewayLog = false }
        ) {
            GatewayLogModal(shell: shell) { shell.showingGatewayLog = false }
        }
    }

    private var hero: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            VStack(alignment: .leading, spacing: Design.Space.s) {
                Text("Gateway")
                    .font(Design.hero)
                    .foregroundStyle(Design.ink)
                Text("One local endpoint for every tool on your Mac.")
                    .font(Design.readingBody)
                    .foregroundStyle(Design.inkSoft)
            }
            Spacer(minLength: 0)
            HStack(spacing: Design.Space.s) {
                QuietIconButton(glyph: "cable.connector") {
                    showingConnect = true
                }
                .help("Connect a tool")
                .accessibilityLabel("Connect a tool")
                .accessibilityIdentifier("gateway-connect")
                QuietIconButton(glyph: "person.badge.plus") {
                    showingAddClient = true
                }
                .help("Add a client")
                .accessibilityLabel("Add a client")
                QuietIconButton(glyph: "list.bullet") {
                    shell.showingGatewayLog = true
                }
                .help("All requests")
                .accessibilityLabel("All requests")
            }
        }
    }

    private var board: some View {
        HStack(alignment: .top, spacing: Design.Space.l) {
            statusCard
            factsCard
        }
    }

    private var statusCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            MicroHeader(title: "Status")
            HStack(spacing: Design.Space.chipX) {
                if running {
                    AccentDot(size: 10)
                } else {
                    RoundedRectangle(cornerRadius: 1)
                        .fill(Design.inkFaint)
                        .frame(width: 10, height: 10)
                }
                Text(running ? "Running" : "Stopped")
                    .font(Design.display)
                    .foregroundStyle(Design.ink)
                Spacer(minLength: 0)
            }
            Text(
                running
                    ? "Listening on loopback · port \(port)"
                    : "No socket exists until you start it."
            )
            .font(Design.readingBody)
            .foregroundStyle(Design.inkSoft)
            if running {
                addressStrip
            }
            if let notice = model.gatewayNotice {
                HStack(spacing: Design.Space.s) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(Design.glyphInline)
                        .foregroundStyle(Design.inkSoft)
                    Text(notice)
                        .font(Design.caption.weight(.medium))
                        .foregroundStyle(Design.inkSoft)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 0)
            HStack(spacing: Design.Space.l) {
                Button(running ? "Stop" : "Start") {
                    model.setGatewayEnabled(!running)
                }
                .buttonStyle(InkButtonStyle())
                .accessibilityIdentifier("gateway-enable")
                HStack(spacing: Design.Space.s) {
                    Text("Port")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                    InkField(placeholder: "43367", text: $portText)
                        .frame(width: 84)
                        .onSubmit { applyPort() }
                        .accessibilityIdentifier("gateway-port")
                }
                Spacer(minLength: 0)
            }
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .surfaceCard(radius: Design.Radius.card)
        .accessibilityIdentifier("gateway-status")
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
            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(address, forType: .string)
                copiedAddress = true
                Task {
                    try? await Task.sleep(for: .seconds(2))
                    copiedAddress = false
                }
            } label: {
                Text(copiedAddress ? "Copied" : "Copy")
                    .font(Design.micro)
                    .tracking(Design.microTracking)
                    .foregroundStyle(copiedAddress ? Design.heatText : Design.inkSoft)
            }
            .buttonStyle(PressDipStyle())
            .accessibilityLabel("Copy gateway address")
        }
        .padding(.horizontal, Design.Space.tile)
        .padding(.vertical, Design.Space.chipX)
        .background(Design.paper, in: RoundedRectangle.soft(Design.Radius.tile))
        .overlay(
            RoundedRectangle.soft(Design.Radius.tile)
                .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        .accessibilityIdentifier("gateway-address")
    }

    private var factsCard: some View {
        VStack(alignment: .leading, spacing: Design.Space.l) {
            MicroHeader(title: "At a glance")
            HStack(alignment: .top, spacing: Design.Space.xl) {
                factTile(
                    "\(model.gatewayClients.count)",
                    model.gatewayClients.count == 1 ? "client" : "clients")
                factTile("\(model.gatewayAuditEntries.count)", "requests")
                factTile("\(GatewayEndpoints.catalog.count)", "endpoints")
            }
            Spacer(minLength: 0)
            Text(glanceCaption)
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(Design.Space.xl)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .surfaceCard(radius: Design.Radius.card)
    }

    private func factTile(_ number: String, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.s) {
            PixelNumber(text: number, unit: 5, color: Design.ink)
                .frame(height: 5 * 7, alignment: .bottomLeading)
            Text(label)
                .font(Design.micro)
                .foregroundStyle(Design.inkFaint)
        }
    }

    private var glanceCaption: String {
        guard running else { return "Start the gateway to serve these on loopback." }
        if model.gatewayClients.isEmpty {
            return "Add a client token before any tool can reach it."
        }
        return "\(activeClientCount) of \(model.gatewayClients.count) have made a request."
    }

    private var activeClientCount: Int {
        model.gatewayClients.count { $0.lastUsedAt != nil }
    }

    private var clientsSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack {
                MicroHeader(title: "Clients · \(model.gatewayClients.count)")
                Spacer(minLength: 0)
                Button("Add a client…") {
                    showingAddClient = true
                }
                .buttonStyle(QuietButtonStyle())
                .accessibilityIdentifier("gateway-add-client")
            }
            clientsCard
        }
    }

    private var clientsCard: some View {
        let auditCounts = Dictionary(grouping: model.gatewayAuditEntries, by: \.client)
            .mapValues(\.count)
        return VStack(alignment: .leading, spacing: 0) {
            if model.gatewayClients.isEmpty {
                Text("Every request needs a token, loopback included. Create one per tool.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.vertical, Design.Space.m)
            } else {
                HStack(spacing: Design.Space.m) {
                    Text("Client")
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Text("Recent")
                        .frame(width: 60, alignment: .trailing)
                    Text("Last used")
                        .frame(width: 116, alignment: .trailing)
                    Color.clear.frame(width: 18, height: 1)
                }
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
                .padding(.vertical, Design.Space.s)
                ForEach(model.gatewayClients) { client in
                    clientRow(client, recent: auditCounts[client.id] ?? 0)
                }
            }
        }
        .padding(.horizontal, Design.Space.tile)
        .padding(.vertical, Design.Space.xs)
        .surfaceCard(radius: Design.Radius.tile)
    }

    private func clientRow(_ client: GatewayClient, recent: Int) -> some View {
        HStack(spacing: Design.Space.m) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: Design.Space.s) {
                    if client.lastUsedAt != nil {
                        AccentDot(size: 7)
                    } else {
                        RoundedRectangle(cornerRadius: 1)
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
            Button {
                model.revokeGatewayClient(id: client.id)
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .font(Design.glyphInline)
                    .foregroundStyle(Design.inkFaint)
            }
            .buttonStyle(PressDipStyle())
            .frame(width: 18)
            .accessibilityLabel("Revoke \(client.name)")
        }
        .padding(.vertical, Design.Space.s)
        .overlay(alignment: .top) {
            Rectangle()
                .fill(Design.line)
                .frame(height: Design.hairlineWidth)
        }
    }

    private var examplesSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack {
                MicroHeader(title: "Connect a tool")
                Spacer(minLength: 0)
                Button("More examples…") {
                    showingConnect = true
                }
                .buttonStyle(QuietButtonStyle())
            }
            VStack(alignment: .leading, spacing: Design.Space.l) {
                GatewayCodeBlock(
                    title: "chat completion",
                    code: GatewayExamples.chatCurl(
                        port: port, model: tryModel,
                        token: GatewayExamples.tokenPlaceholder))
                GatewayCodeBlock(
                    title: "list models",
                    code: GatewayExamples.modelsCurl(
                        port: port, token: GatewayExamples.tokenPlaceholder))
            }
        }
    }

    private var tryModel: String {
        let ready = shell.library.records.filter { $0.state == .ready }
        return ready.first { $0.capabilities.contains(.chat) }?.displayName
            ?? ready.first?.displayName ?? "your-model"
    }

    private var endpointsSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Endpoints")
            VStack(alignment: .leading, spacing: Design.Space.m) {
                ForEach(Array(GatewayEndpoints.grouped.enumerated()), id: \.offset) { _, section in
                    Text(section.group)
                        .font(Design.micro)
                        .tracking(Design.microTracking)
                        .foregroundStyle(Design.inkFaint)
                        .padding(.top, Design.Space.xs)
                    ForEach(section.endpoints) { endpoint in
                        endpointRow(endpoint)
                    }
                }
                if !running {
                    Text("Turn the gateway on above to reach these.")
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                        .padding(.top, Design.Space.xs)
                }
            }
            .padding(.horizontal, Design.Space.tile)
            .padding(.vertical, Design.Space.tile)
            .surfaceCard(radius: Design.Radius.tile)
        }
    }

    private func endpointRow(_ endpoint: GatewayEndpointInfo) -> some View {
        HStack(spacing: Design.Space.s) {
            Text(endpoint.method)
                .font(Design.micro)
                .tracking(Design.microTracking)
                .foregroundStyle(Design.inkSoft)
                .frame(width: 44, alignment: .leading)
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

    private var auditSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            HStack(spacing: Design.Space.m) {
                MicroHeader(title: "Recent requests")
                Spacer(minLength: 0)
                if model.gatewayAuditEntries.count > 12 {
                    Button("Show all \(model.gatewayAuditEntries.count)") {
                        shell.showingGatewayLog = true
                    }
                    .buttonStyle(QuietButtonStyle())
                }
                Button("Show in Finder") {
                    NSWorkspace.shared.activateFileViewerSelecting([model.gatewayAuditFileURL])
                }
                .buttonStyle(QuietButtonStyle())
            }
            auditCard
        }
    }

    private var auditCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            if model.gatewayAuditEntries.isEmpty {
                Text("Requests land here — successes and refusals both.")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
                    .padding(.vertical, Design.Space.m)
            } else {
                ForEach(Array(model.gatewayAuditEntries.prefix(12).enumerated()), id: \.offset) {
                    index, entry in
                    auditRow(entry, first: index == 0)
                }
            }
        }
        .padding(.horizontal, Design.Space.tile)
        .padding(.vertical, Design.Space.xs)
        .surfaceCard(radius: Design.Radius.tile)
    }

    private func auditRow(_ entry: GatewayAuditEntry, first: Bool) -> some View {
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
            if !first {
                Rectangle()
                    .fill(Design.line)
                    .frame(height: Design.hairlineWidth)
            }
        }
    }

    private func applyPort() {
        guard let value = Int(portText), (1024...65535).contains(value) else {
            portText = String(model.gateway.port)
            return
        }
        model.gateway.port = value
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

    private func lastUsedShort(_ client: GatewayClient) -> String {
        guard let used = client.lastUsedAt else { return "never" }
        return Self.relative.localizedString(for: used, relativeTo: Date())
    }

    private static let time: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()

    private static let relative = RelativeDateTimeFormatter()
}

struct GatewayLogModal: View {
    @Bindable var shell: ShellModel
    let onClose: () -> Void

    private var model: SettingsModel { shell.settings }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .center, spacing: Design.Space.l) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("All requests")
                        .font(Design.title)
                        .foregroundStyle(Design.ink)
                    Text("\(model.gatewayAuditEntries.count) recorded")
                        .font(Design.data(11))
                        .foregroundStyle(Design.inkFaint)
                }
                Spacer(minLength: 0)
                SheetCloseButton(action: onClose)
            }
            .padding(Design.Space.gutter)
            Rectangle()
                .fill(Design.hairline)
                .frame(height: Design.hairlineWidth)
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    ForEach(Array(model.gatewayAuditEntries.enumerated()), id: \.offset) {
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
                            if let name = entry.clientName {
                                Text(name)
                                    .font(Design.label)
                                    .foregroundStyle(Design.inkFaint)
                                    .lineLimit(1)
                                    .frame(width: 120, alignment: .trailing)
                            }
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
                        Text("No requests yet.")
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                            .padding(.vertical, Design.Space.m)
                    }
                }
                .padding(.horizontal, Design.Space.gutter)
                .padding(.vertical, Design.Space.m)
            }
        }
        .frame(width: 720, height: 620)
    }

    private static let time: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }()
}
