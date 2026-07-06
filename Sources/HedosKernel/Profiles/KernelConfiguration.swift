extension Kernel {
    public func setParamValue(_ modelID: String, key: String, to value: JSONValue?) async throws {
        var record = try await requireModel(modelID)
        if let value, value != .null {
            guard let spec = record.params.first(where: { $0.key == key }),
                let normalized = spec.normalized(value)
            else {
                throw KernelError.paramUnsupported(model: record.name, key: key)
            }
            record.paramValues[key] = normalized
        } else {
            record.paramValues[key] = nil
        }
        try await registry.register(record)
    }

    public func resetParamValues(_ modelID: String) async throws {
        var record = try await requireModel(modelID)
        record.paramValues = [:]
        try await registry.register(record)
    }

    public func setSystemPrompt(_ modelID: String, to prompt: String?) async throws {
        var record = try await requireModel(modelID)
        let trimmed = prompt?.trimmingCharacters(in: .whitespacesAndNewlines)
        record.systemPrompt = (trimmed?.isEmpty ?? true) ? nil : trimmed
        try await registry.register(record)
    }

    public func setAlias(_ modelID: String, to alias: String?) async throws {
        var record = try await requireModel(modelID)
        let trimmed = alias?.trimmingCharacters(in: .whitespacesAndNewlines)
        record.alias = (trimmed?.isEmpty ?? true) ? nil : trimmed
        try await registry.register(record)
    }

    func requireModel(_ modelID: String) async throws -> ModelRecord {
        guard let record = try await registry.get(id: modelID) else {
            throw KernelError.modelNotFound(modelID)
        }
        return record
    }
}
