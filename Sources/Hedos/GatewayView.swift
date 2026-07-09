import AppKit
import HedosKernel
import SwiftUI

struct GatewayPane: View {
    @Bindable var shell: ShellModel
    @State private var showingAddClient = false
    @State private var showingConnect = false

    private var status: GatewayStatus { shell.settings.gatewayStatus }

    private var statusText: String {
        status.running
            ? "live · :\(String(status.port ?? shell.settings.gateway.port))"
            : "offline"
    }

    var body: some View {
        VStack(spacing: 0) {
            PaneHeader(title: "Gateway") {
                TintChip(text: statusText, live: status.running, faint: !status.running)
                    .accessibilityIdentifier("gateway-status")
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
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ScrollView {
                GatewaySection(
                    shell: shell,
                    highlighted: nil,
                    onAddClient: { showingAddClient = true },
                    onConnect: { showingConnect = true },
                    onShowAllRequests: { shell.showingGatewayLog = true },
                    showsControlHeader: false
                )
                .padding(.horizontal, Design.Space.gutter + Design.Space.m)
                .padding(.top, Design.Space.xl)
                .padding(.bottom, Design.Space.pane)
                .frame(maxWidth: Design.Column.hero, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
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
