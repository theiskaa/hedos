import HedosKernel
import SwiftUI

struct OrchestraMenu: View {
    let library: LibraryViewModel
    let model: ChatViewModel
    let kernel: Kernel
    let onClose: () -> Void
    let onInstallModels: () -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.keyNav) private var keyNav
    @State private var savedAsDefault = false
    @State private var saveFailed = false
    @State private var toolSupport: [String: Bool] = [:]
    @State private var page: Page = .overview

    private enum Page: Equatable {
        case overview
        case main
        case role(String)
    }

    private struct Role: Identifiable {
        let id: String
        let title: String
        let detail: String
        let glyph: String
        let capability: Capability
    }

    private static let roles: [Role] = [
        Role(
            id: "images", title: "Images",
            detail: "Draws when the conversation asks for an image.",
            glyph: "photo", capability: .image),
        Role(
            id: "voice", title: "Voice",
            detail: "Speaks replies and narrations aloud.",
            glyph: "speaker.wave.2", capability: .speak),
        Role(
            id: "eyes", title: "Eyes",
            detail: "Looks at images and reports what they actually show.",
            glyph: "eye", capability: .see),
    ]

    var body: some View {
        ZStack(alignment: .top) {
            switch page {
            case .overview:
                overview
                    .transition(.arrive(from: .leading, reduceMotion: reduceMotion))
            case .main:
                mainPage
                    .transition(.arrive(from: .trailing, reduceMotion: reduceMotion))
            case .role(let id):
                if let role = Self.roles.first(where: { $0.id == id }) {
                    rolePage(role)
                        .transition(.arrive(from: .trailing, reduceMotion: reduceMotion))
                }
            }
        }
        .padding(Design.Space.s)
        .animation(Design.motion(reduceMotion: reduceMotion), value: page)
        .environment(\.inkMenuDismiss) { onClose() }
        .onAppear { syncKeyHooks() }
        .onChange(of: page) { _, _ in syncKeyHooks() }
        .task(id: library.shelfSignature) {
            var probed: [String: Bool] = [:]
            let candidates = mainCandidates.map(\.id) + [model.boundModelID].compactMap { $0 }
            for id in Set(candidates) {
                probed[id] = (try? await kernel.supportsTools(modelID: id)) ?? false
            }
            toolSupport = probed
        }
    }

    private var overview: some View {
        VStack(alignment: .leading, spacing: Design.Space.xxs) {
            if noMainIsSelectable {
                InkMenuRow(title: "Install a model…", glyph: "arrow.down.circle") {
                    onInstallModels()
                }
            } else {
                InkMenuRow(
                    title: "Main",
                    annotation: mainRecord?.displayName ?? "Choose a model",
                    glyph: "person.wave.2",
                    dismisses: false,
                    chevron: true
                ) {
                    page = .main
                }
                .help("Runs the conversation and decides when to call the others.")
                if let id = model.boundModelID, let record = library.record(id: id),
                    toolSupport[id] == false
                {
                    caption(
                        "\(record.displayName) can't call other models — pick a "
                            + "different main model to make the orchestra play.")
                }
            }
            divider
            ForEach(Self.roles) { role in
                InkMenuRow(
                    title: role.title,
                    annotation: member(for: role.capability)?.displayName ?? "None",
                    glyph: role.glyph,
                    dismisses: false,
                    chevron: true
                ) {
                    page = .role(role.id)
                }
                .help(role.detail)
            }
            unavailableSection
            caption("Every call the main model makes shows in the conversation.")
            divider
            InkMenuRow(
                title: saveTitle,
                glyph: "pin",
                disabled: savedAsDefault,
                dismisses: false
            ) {
                saveAsDefault()
            }
        }
    }

    private var mainPage: some View {
        VStack(alignment: .leading, spacing: Design.Space.xxs) {
            backRow("Main")
            divider
            ForEach(mainCandidates) { record in
                InkMenuRow(
                    title: record.displayName,
                    annotation: toolSupport[record.id] == false ? "can't use tools" : nil,
                    selected: record.id == model.boundModelID,
                    disabled: toolSupport[record.id] == false,
                    dismisses: false
                ) {
                    model.rebind(to: record)
                    savedAsDefault = false
                    page = .overview
                }
            }
        }
    }

    private func rolePage(_ role: Role) -> some View {
        let candidates = candidates(for: role.capability)
        let current = member(for: role.capability)
        return VStack(alignment: .leading, spacing: Design.Space.xxs) {
            backRow(role.title)
            caption(role.detail)
            divider
            if candidates.isEmpty {
                InkMenuRow(title: "Install a model…", glyph: "arrow.down.circle") {
                    onInstallModels()
                }
            } else {
                InkMenuRow(title: "None", selected: current == nil, dismisses: false) {
                    assign(role.capability, to: nil)
                    page = .overview
                }
                ForEach(candidates) { record in
                    InkMenuRow(
                        title: record.displayName,
                        selected: record.id == current?.id,
                        dismisses: false
                    ) {
                        assign(role.capability, to: record)
                        page = .overview
                    }
                }
            }
        }
    }

    private func syncKeyHooks() {
        guard let keyNav else { return }
        if page == .overview {
            keyNav.escapeOverride = nil
            keyNav.leftOverride = nil
        } else {
            let back: () -> Bool = {
                page = .overview
                return true
            }
            keyNav.escapeOverride = back
            keyNav.leftOverride = back
        }
    }

    private func backRow(_ title: String) -> some View {
        InkMenuRow(title: title, glyph: "chevron.left", dismisses: false) {
            page = .overview
        }
    }

    private func caption(_ text: String) -> some View {
        Text(text)
            .font(Design.micro)
            .foregroundStyle(Design.inkFaint)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.horizontal, Design.Space.chipX)
            .padding(.vertical, Design.Space.xs)
    }

    private var divider: some View {
        Rectangle()
            .fill(Design.line)
            .frame(height: Design.hairlineWidth)
            .padding(.vertical, Design.Space.xxs)
    }

    @ViewBuilder
    private var unavailableSection: some View {
        ForEach(unavailableIDs, id: \.self) { id in
            InkMenuRow(
                title: library.records.first { $0.id == id }?.displayName ?? id,
                annotation: "unavailable — tap to remove",
                glyph: "exclamationmark.triangle",
                dismisses: false
            ) {
                model.setBench(model.bench.filter { $0 != id })
                savedAsDefault = false
            }
        }
    }

    private var saveTitle: String {
        if savedAsDefault { return "Saved for new chats" }
        if saveFailed { return "Saving failed — try again" }
        return "Use for new chats"
    }

    private var mainRecord: ModelRecord? {
        library.record(id: model.boundModelID)
    }

    private var mainCandidates: [ModelRecord] {
        library.records.filter { $0.state == .ready && $0.capabilities.contains(.chat) }
    }

    private var noMainIsSelectable: Bool {
        mainCandidates.allSatisfy { toolSupport[$0.id] == false }
    }

    private func candidates(for capability: Capability) -> [ModelRecord] {
        library.records.filter { $0.state == .ready && $0.capabilities.contains(capability) }
    }

    private func member(for capability: Capability) -> ModelRecord? {
        model.bench
            .compactMap { id in library.records.first { $0.id == id } }
            .first { $0.state == .ready && $0.capabilities.contains(capability) }
    }

    private var unavailableIDs: [String] {
        model.bench.filter { id in
            guard let record = library.records.first(where: { $0.id == id }) else { return true }
            return record.state != .ready
        }
    }

    private func assign(_ capability: Capability, to record: ModelRecord?) {
        model.setBench(
            BenchTools.assigning(
                capability, to: record?.id, in: model.bench, records: library.records))
        savedAsDefault = false
    }

    private func saveAsDefault() {
        let mainID = model.boundModelID
        let bench = model.bench
        let kernel = kernel
        Task {
            do {
                if let mainID {
                    try await kernel.settings.setDefaultChatModelID(mainID)
                }
                var chat = await kernel.settings.chat()
                chat.defaultBench = bench
                try await kernel.settings.save(chat)
                savedAsDefault = true
                saveFailed = false
            } catch {
                savedAsDefault = false
                saveFailed = true
            }
        }
    }
}
