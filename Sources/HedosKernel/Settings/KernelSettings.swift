extension Kernel {
    public func generalSettings() async -> GeneralSettings {
        await settings.general()
    }

    public func updateGeneralSettings(_ value: GeneralSettings) async throws {
        try await settings.save(value)
    }

    public func modelsSettings() async -> ModelsSettings {
        await settings.models()
    }

    public func updateModelsSettings(_ value: ModelsSettings) async throws {
        try await settings.save(value)
        await applyStoredPolicies()
    }

    public func chatSettings() async -> ChatSettings {
        await settings.chat()
    }

    public func updateChatSettings(_ value: ChatSettings) async throws {
        try await settings.save(value)
    }

    public func voiceSettings() async -> VoiceSettings {
        await settings.voice()
    }

    public func updateVoiceSettings(_ value: VoiceSettings) async throws {
        try await settings.save(value)
    }

    public func appearanceSettings() async -> AppearanceSettings {
        await settings.appearance()
    }

    public func updateAppearanceSettings(_ value: AppearanceSettings) async throws {
        try await settings.save(value)
    }

    public func advancedSettings() async -> AdvancedSettings {
        await settings.advanced()
    }

    public func updateAdvancedSettings(_ value: AdvancedSettings) async throws {
        try await settings.save(value)
        await applyStoredPolicies()
    }

    public func applyStoredPolicies() async {
        await governor.apply(policy: settings.models().residencyPolicy)
        await scheduler.history.setLimit(settings.advanced().jobHistoryLimit)
    }
}
