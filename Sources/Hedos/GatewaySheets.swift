import AppKit
import HedosKernel
import SwiftUI

struct AddGatewayClientSheet: View {
    let shell: ShellModel
    let onClose: () -> Void

    @State private var name = ""
    @State private var allModels = true
    @State private var pickedModels: Set<String> = []
    @State private var allCapabilities = true
    @State private var pickedCapabilities: Set<String> = []
    @State private var creation: GatewayClientCreation?
    @State private var creating = false
    @State private var copied = false

    private static let capabilityChoices = ["chat", "speak", "image"]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
                .padding(.horizontal, Design.Space.gutter)
                .padding(.top, Design.Space.gutter)
                .padding(.bottom, Design.Space.xl)
            Rectangle().fill(Design.hairline).frame(height: Design.hairlineWidth)
            ScrollView {
                VStack(alignment: .leading, spacing: Design.Space.xl) {
                    if let creation {
                        tokenSection(creation)
                    } else {
                        nameSection
                        modelsSection
                        capabilitiesSection
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
                Image(systemName: "key")
                    .font(Design.glyphNav)
                    .foregroundStyle(Design.inkSoft)
            }
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(creation == nil ? "Add a client" : "Client token")
                    .font(Design.title)
                    .tracking(Design.tightTracking)
                Text(
                    creation == nil
                        ? "A scoped token for one tool or script"
                        : "Copy it now — it is never shown again"
                )
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
            }
            Spacer()
            SheetCloseButton(action: onClose)
        }
    }

    private var nameSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Name")
            InkField(placeholder: "my editor", text: $name)
                .accessibilityIdentifier("gateway-client-name")
        }
    }

    private var modelsSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Models")
            InkSegmented(
                values: ["All models", "Selected"],
                selection: allModels ? "All models" : "Selected",
                onSelect: { allModels = $0 == "All models" })
            if !allModels {
                VStack(alignment: .leading, spacing: Design.Space.xs) {
                    ForEach(readyModels, id: \.id) { record in
                        choiceRow(
                            title: record.displayName,
                            selected: pickedModels.contains(record.id)
                        ) {
                            if pickedModels.contains(record.id) {
                                pickedModels.remove(record.id)
                            } else {
                                pickedModels.insert(record.id)
                            }
                        }
                    }
                    if readyModels.isEmpty {
                        Text("No ready models on the shelf yet.")
                            .font(Design.label)
                            .foregroundStyle(Design.inkFaint)
                    }
                }
                .padding(Design.Space.tile)
                .surfaceCard(radius: Design.Radius.tile)
            }
        }
    }

    private var capabilitiesSection: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Capabilities")
            InkSegmented(
                values: ["All capabilities", "Selected"],
                selection: allCapabilities ? "All capabilities" : "Selected",
                onSelect: { allCapabilities = $0 == "All capabilities" })
            if !allCapabilities {
                HStack(spacing: Design.Space.s) {
                    ForEach(Self.capabilityChoices, id: \.self) { capability in
                        choiceChip(
                            title: capability,
                            selected: pickedCapabilities.contains(capability)
                        ) {
                            if pickedCapabilities.contains(capability) {
                                pickedCapabilities.remove(capability)
                            } else {
                                pickedCapabilities.insert(capability)
                            }
                        }
                    }
                }
            }
        }
    }

    private func tokenSection(_ creation: GatewayClientCreation) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            MicroHeader(title: "Token for \(creation.client.name)")
            HStack(spacing: Design.Space.s) {
                Text(creation.token)
                    .font(Design.data(12))
                    .foregroundStyle(Design.ink)
                    .textSelection(.enabled)
                    .lineLimit(2)
                    .truncationMode(.middle)
                    .accessibilityIdentifier("gateway-token")
                Spacer()
                Button(copied ? "Copied" : "Copy") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(creation.token, forType: .string)
                    copied = true
                }
                .buttonStyle(InkButtonStyle())
            }
            .padding(Design.Space.tile)
            .surfaceCard(radius: Design.Radius.tile)
            Text("Hedos keeps only a hash. Losing this token means minting a new client.")
                .font(Design.label)
                .foregroundStyle(Design.inkFaint)
            GatewayCodeBlock(
                title: "try it",
                code: GatewayExamples.chatCurl(
                    port: gatewayPort, model: tryModel, token: creation.token))
        }
    }

    private var gatewayPort: Int {
        shell.settings.gatewayStatus.port ?? shell.settings.gateway.port
    }

    private var tryModel: String {
        let ready = shell.library.records.filter { $0.state == .ready }
        return ready.first { $0.capabilities.contains(.chat) }?.displayName
            ?? ready.first?.displayName ?? "your-model"
    }

    private var footer: some View {
        HStack {
            Spacer()
            if creation == nil {
                Button(creating ? "Creating…" : "Create") {
                    create()
                }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || creating)
                .accessibilityIdentifier("gateway-client-create")
            } else {
                Button("Done") {
                    onClose()
                }
                .buttonStyle(InkButtonStyle())
                .keyboardShortcut(.defaultAction)
            }
        }
    }

    private var readyModels: [ModelRecord] {
        shell.library.records.filter { $0.state == .ready }
    }

    private func choiceRow(
        title: String, selected: Bool, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: Design.Space.s) {
                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                    .font(Design.glyphInline)
                    .foregroundStyle(selected ? Design.ink : Design.inkFaint)
                Text(title)
                    .font(Design.label)
                    .foregroundStyle(Design.ink)
                    .lineLimit(1)
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func choiceChip(
        title: String, selected: Bool, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Text(title)
                .font(Design.caption.weight(.medium))
                .foregroundStyle(selected ? Design.paper : Design.ink)
                .padding(.horizontal, Design.Space.m)
                .padding(.vertical, Design.Space.xs)
                .background(
                    RoundedRectangle(cornerRadius: Design.Radius.control).fill(selected ? Design.ink : Design.paper))
                .overlay(
                    RoundedRectangle(cornerRadius: Design.Radius.control).strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func create() {
        creating = true
        let scopes = GatewayScopes(
            models: allModels ? nil : Array(pickedModels).sorted(),
            capabilities: allCapabilities ? nil : Array(pickedCapabilities).sorted())
        let trimmed = name.trimmingCharacters(in: .whitespaces)
        let shell = shell
        Task { @MainActor in
            creation = await shell.settings.createGatewayClient(name: trimmed, scopes: scopes)
            creating = false
        }
    }
}
