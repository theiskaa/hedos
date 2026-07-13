import HedosKernel

extension ShellModel {
    func toggleCommandPalette() {
        commandPaletteOpen.toggle()
    }

    func renameChat(id: String, to title: String) {
        let trimmed = title.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return }
        if let index = sessions.firstIndex(where: { $0.id == id }) {
            sessions[index].title = trimmed
        }
        let kernel = kernel
        Task {
            try? await kernel.chats.renameSession(id: id, title: trimmed)
            await refreshSessions()
        }
    }

    func setChatPinned(id: String, _ pinned: Bool) {
        if let index = sessions.firstIndex(where: { $0.id == id }) {
            sessions[index].pinned = pinned
        }
        let kernel = kernel
        Task {
            try? await kernel.chats.setPinned(id: id, pinned)
            await refreshSessions()
        }
    }

    func setChatArchived(id: String, _ archived: Bool) {
        if let index = sessions.firstIndex(where: { $0.id == id }) {
            sessions[index].archived = archived
        }
        if archived, chatSelection == id {
            selectChat(nil)
        }
        let kernel = kernel
        Task {
            try? await kernel.chats.setArchived(id: id, archived)
            await refreshSessions()
        }
    }

    func deleteChat(id: String) {
        let kernel = kernel
        Task {
            do {
                try await kernel.chats.deleteSession(id: id)
                discardChatModel(id)
                if chatSelection == id {
                    selectChat(nil)
                }
            } catch {}
            await refreshSessions()
        }
    }
}
