import HedosKernel
import SwiftUI

struct OrchestraSheet: View {
    let library: LibraryViewModel
    let model: ChatViewModel
    let kernel: Kernel
    let onClose: () -> Void
    let onInstallModels: () -> Void
    @State private var savedAsDefault = false
    @State private var saveFailed = false
    @State private var toolSupport: [String: Bool] = [:]

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
        sheetContent
            .frame(width: Design.Sheet.orchestraWidth, alignment: .leading)
            .task(id: library.shelfSignature) {
                var probed: [String: Bool] = [:]
                let candidates = mainCandidates.map(\.id) + [model.boundModelID].compactMap { $0 }
                for id in Set(candidates) {
                    probed[id] = (try? await kernel.supportsTools(modelID: id)) ?? false
                }
                toolSupport = probed
            }
    }

    private var sheetContent: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Orchestra")
                .font(Design.title)
                .foregroundStyle(Design.ink)
            Text("The main model runs the conversation and plays the others as instruments.")
                .font(Design.caption)
                .foregroundStyle(Design.inkSoft)
                .padding(.top, Design.Space.xxs)
            mainRow
                .padding(.top, Design.Space.xl)
            if let id = model.boundModelID, let record = library.record(id: id),
                toolSupport[id] == false
            {
                HStack(spacing: Design.Space.s) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(Design.glyphSmall)
                        .foregroundStyle(Design.inkFaint)
                        .frame(width: 20)
                    Text(
                        "\(record.displayName) can't call other models — pick a different "
                            + "main model to make the orchestra play."
                    )
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
                    .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.top, Design.Space.xs)
            }
            Rectangle()
                .fill(Design.line)
                .frame(height: Design.hairlineWidth)
                .padding(.vertical, Design.Space.s)
            ForEach(Self.roles) { role in
                roleRow(role)
            }
            if !unavailableIDs.isEmpty {
                unavailableRows
                    .padding(.top, Design.Space.s)
            }
            Text("Every call the main model makes shows in the conversation.")
                .font(Design.caption)
                .foregroundStyle(Design.inkFaint)
                .padding(.top, Design.Space.l)
            HStack(spacing: Design.Space.m) {
                Button(saveButtonTitle) {
                    saveAsDefault()
                }
                .buttonStyle(QuietButtonStyle())
                .disabled(savedAsDefault)
                Spacer(minLength: Design.Space.l)
                Button("Done", action: onClose)
                    .buttonStyle(QuietButtonStyle())
                    .keyboardShortcut(.defaultAction)
            }
            .padding(.top, Design.Space.xl)
        }
        .padding(Design.Space.gutter)
        .animation(Design.wash, value: savedAsDefault)
        .animation(Design.wash, value: model.bench)
    }

    private var saveButtonTitle: String {
        if savedAsDefault { return "Saved for new chats" }
        if saveFailed { return "Saving failed — try again" }
        return "Use for new chats"
    }

    private var mainRow: some View {
        row(
            glyph: "person.wave.2",
            title: "Main",
            detail: "Runs the conversation and decides when to call the others."
        ) {
            if noMainIsSelectable {
                Button("Install a model…", action: onInstallModels)
                    .buttonStyle(QuietButtonStyle())
            } else {
                InkMenu(
                    title: mainTitle,
                    accessibilityName: "Main model",
                    readyDot: mainRecord?.state == .ready
                ) {
                    ForEach(mainCandidates) { record in
                        InkMenuRow(
                            title: record.displayName,
                            annotation: toolSupport[record.id] == false
                                ? "can't use tools" : nil,
                            selected: record.id == model.boundModelID,
                            disabled: toolSupport[record.id] == false
                        ) {
                            model.rebind(to: record)
                            savedAsDefault = false
                        }
                    }
                }
            }
        }
    }

    private var noMainIsSelectable: Bool {
        mainCandidates.allSatisfy { toolSupport[$0.id] == false }
    }

    private func roleRow(_ role: Role) -> some View {
        let candidates = candidates(for: role.capability)
        let current = member(for: role.capability)
        return row(glyph: role.glyph, title: role.title, detail: role.detail) {
            if candidates.isEmpty {
                Button("Install a model…", action: onInstallModels)
                    .buttonStyle(QuietButtonStyle())
            } else {
                InkMenu(
                    title: current?.displayName ?? "None",
                    accessibilityName: "\(role.title) model"
                ) {
                    InkMenuRow(title: "None", selected: current == nil) {
                        assign(role.capability, to: nil)
                    }
                    ForEach(candidates) { record in
                        InkMenuRow(
                            title: record.displayName,
                            selected: record.id == current?.id
                        ) {
                            assign(role.capability, to: record)
                        }
                    }
                }
            }
        }
    }

    private var unavailableRows: some View {
        VStack(alignment: .leading, spacing: Design.Space.xs) {
            ForEach(unavailableIDs, id: \.self) { id in
                HStack(spacing: Design.Space.s) {
                    Image(systemName: "exclamationmark.triangle")
                        .font(Design.glyphSmall)
                        .foregroundStyle(Design.inkFaint)
                        .frame(width: 20)
                    Text(library.records.first { $0.id == id }?.displayName ?? id)
                        .font(Design.caption)
                        .foregroundStyle(Design.inkSoft)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Text("no longer available")
                        .font(Design.micro)
                        .foregroundStyle(Design.inkFaint)
                    Spacer(minLength: Design.Space.m)
                    Button("Remove") {
                        model.setBench(model.bench.filter { $0 != id })
                        savedAsDefault = false
                    }
                    .buttonStyle(QuietButtonStyle())
                }
            }
        }
    }

    private func row<Picker: View>(
        glyph: String, title: String, detail: String, @ViewBuilder picker: () -> Picker
    ) -> some View {
        HStack(alignment: .center, spacing: Design.Space.l) {
            Image(systemName: glyph)
                .font(Design.glyphInline)
                .foregroundStyle(Design.inkSoft)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: Design.Space.xxs) {
                Text(title)
                    .font(Design.body.weight(.medium))
                    .foregroundStyle(Design.ink)
                Text(detail)
                    .font(Design.caption)
                    .foregroundStyle(Design.inkFaint)
                    .lineSpacing(1.5)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: Design.Space.l)
            picker()
        }
        .padding(.vertical, Design.Space.s)
    }

    private var mainRecord: ModelRecord? {
        library.record(id: model.boundModelID)
    }

    private var mainTitle: String {
        mainRecord?.displayName ?? "Choose a model"
    }

    private var mainCandidates: [ModelRecord] {
        library.records.filter { $0.state == .ready && $0.capabilities.contains(.chat) }
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
        var ids = model.bench
        if let current = member(for: capability) {
            ids.removeAll { $0 == current.id }
        }
        if let record {
            ids.removeAll { $0 == record.id }
            ids.append(record.id)
        }
        model.setBench(ids)
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
