extension Kernel {
    public func prompts() async -> [Prompt] {
        await promptStore.list()
    }

    public func prompt(id: String) async -> Prompt? {
        await promptStore.get(id: id)
    }

    @discardableResult
    public func savePrompt(_ prompt: Prompt) async throws -> Prompt {
        try await promptStore.save(prompt)
    }

    public func deletePrompt(id: String) async {
        await promptStore.delete(id: id)
    }

    public func resolvePrompt(
        id: String, placeholders: [String: String] = [:]
    ) async throws -> String {
        guard let prompt = await promptStore.get(id: id) else {
            throw KernelError.promptNotFound(id)
        }
        return prompt.resolvedBody(placeholders)
    }
}
