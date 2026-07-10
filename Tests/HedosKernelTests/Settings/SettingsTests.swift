import Foundation
import Testing

@testable import HedosKernel

private func waitUntil(
    _ condition: @Sendable () async throws -> Bool
) async throws {
    for _ in 0..<500 {
        if try await condition() { return }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("condition never became true")
}

@Test func everyDomainRoundTripsAcrossStoreReload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    var general = GeneralSettings()
    general.restoreLastSession = false
    var models = ModelsSettings()
    models.watchedFolders = ["/tmp/models"]
    models.keepWarm = .oneHour
    models.eviction = .budgeted
    models.ramBudgetMB = 24000
    var chat = ChatSettings()
    chat.defaultModelID = "abc123"
    chat.defaultSystemPrompt = "Be concise."
    var voice = VoiceSettings()
    voice.defaultVoice = "bf_alpha"
    voice.speed = 1.4
    voice.autoSpeak = true
    var appearance = AppearanceSettings()
    appearance.theme = .dark
    appearance.chatWidth = .wide
    appearance.density = .compact
    appearance.uiFont = "Avenir Next"
    appearance.monoFont = "Menlo"
    var advanced = AdvancedSettings()
    advanced.jobHistoryLimit = 200

    try await store.save(general)
    try await store.save(models)
    try await store.save(chat)
    try await store.save(voice)
    try await store.save(appearance)
    try await store.save(advanced)

    for name in ["general", "models", "chat", "voice", "appearance", "advanced"] {
        let file = dir.appendingPathComponent("settings/\(name).json")
        #expect(FileManager.default.fileExists(atPath: file.path))
    }

    let reloaded = SettingsStore(directory: dir)
    #expect(await reloaded.general() == general)
    #expect(await reloaded.models() == models)
    #expect(await reloaded.chat() == chat)
    #expect(await reloaded.voice() == voice)
    #expect(await reloaded.appearance() == appearance)
    #expect(await reloaded.advanced() == advanced)
}

@Test func missingFilesYieldDefaultsAndAreNeverWritten() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    #expect(await store.general() == GeneralSettings())
    #expect(await store.models() == ModelsSettings())
    #expect(await store.chat() == ChatSettings())
    #expect(await store.voice() == VoiceSettings())
    #expect(await store.appearance() == AppearanceSettings())
    #expect(await store.advanced() == AdvancedSettings())

    let settingsDir = dir.appendingPathComponent("settings")
    #expect(!FileManager.default.fileExists(atPath: settingsDir.path))
}

@Test func legacyWatchedFoldersMigrateThroughCompatibilityRead() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let legacy = """
        {"schemaVersion": 1, "watchedFolders": ["/tmp/models-a", "/tmp/models-b"]}
        """
    try legacy.write(
        to: dir.appendingPathComponent("settings.json"), atomically: true, encoding: .utf8)

    let store = SettingsStore(directory: dir)
    let models = await store.models()
    #expect(models.watchedFolders == ["/tmp/models-a", "/tmp/models-b"])
    #expect(models.keepWarm == .fiveMinutes)
    let modelsFile = dir.appendingPathComponent("settings/models.json")
    #expect(!FileManager.default.fileExists(atPath: modelsFile.path))

    let settings = try await store.addWatchedFolder("/tmp/models-c")
    #expect(settings.watchedFolders == ["/tmp/models-a", "/tmp/models-b", "/tmp/models-c"])
    #expect(FileManager.default.fileExists(atPath: modelsFile.path))

    let reloaded = await SettingsStore(directory: dir).models()
    #expect(reloaded.watchedFolders == settings.watchedFolders)
}

@Test func lenientDecodeSurvivesUnknownMissingAndMistypedFields() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let settingsDir = dir.appendingPathComponent("settings")
    try FileManager.default.createDirectory(at: settingsDir, withIntermediateDirectories: true)

    try """
        {"defaultModelID": 42, "futureKnob": true}
        """.write(
        to: settingsDir.appendingPathComponent("chat.json"), atomically: true, encoding: .utf8)
    try """
        {"speed": "fast", "autoSpeak": true, "somethingElse": {"nested": 1}}
        """.write(
        to: settingsDir.appendingPathComponent("voice.json"), atomically: true, encoding: .utf8)
    try """
        {"keepWarm": "2days", "eviction": "budgeted", "watchedFolders": ["/tmp/x"]}
        """.write(
        to: settingsDir.appendingPathComponent("models.json"), atomically: true, encoding: .utf8)
    try "not json at all".write(
        to: settingsDir.appendingPathComponent("appearance.json"), atomically: true,
        encoding: .utf8)

    let store = SettingsStore(directory: dir)
    let chat = await store.chat()
    #expect(chat.defaultModelID == nil)
    #expect(chat.defaultSystemPrompt == nil)

    let voice = await store.voice()
    #expect(voice.speed == 1.0)
    #expect(voice.autoSpeak == true)

    let models = await store.models()
    #expect(models.keepWarm == .fiveMinutes)
    #expect(models.eviction == .budgeted)
    #expect(models.watchedFolders == ["/tmp/x"])

    #expect(await store.appearance() == AppearanceSettings())
}

@Test func watchedFolderRejectsHomeAndRootButAllowsSubfolder() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)
    let home = FileManager.default.homeDirectoryForCurrentUser.path

    await #expect(throws: (any Error).self) {
        _ = try await store.addWatchedFolder("~")
    }
    await #expect(throws: (any Error).self) {
        _ = try await store.addWatchedFolder(home)
    }
    await #expect(throws: (any Error).self) {
        _ = try await store.addWatchedFolder("/")
    }
    await #expect(throws: (any Error).self) {
        _ = try await store.addWatchedFolder("~/Downloads/../..")
    }

    let settings = try await store.addWatchedFolder("~/Downloads/models")
    #expect(settings.watchedFolders.count == 1)
    #expect(settings.watchedFolders[0].hasSuffix("/Downloads/models"))

    let symlinkDir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: symlinkDir) }
    let symlinkToHome = symlinkDir.appendingPathComponent("home-link")
    try FileManager.default.createSymbolicLink(
        at: symlinkToHome, withDestinationURL: FileManager.default.homeDirectoryForCurrentUser)
    await #expect(throws: (any Error).self) {
        _ = try await store.addWatchedFolder(symlinkToHome.path)
    }

    let caseInsensitiveVolume =
        (try? FileManager.default.homeDirectoryForCurrentUser.resourceValues(
            forKeys: [.volumeSupportsCaseSensitiveNamesKey]
        ).volumeSupportsCaseSensitiveNames) == false
    if caseInsensitiveVolume {
        await #expect(throws: (any Error).self) {
            _ = try await store.addWatchedFolder(home.uppercased())
        }
    }
}

@Test func watchedFolderAddRemoveDedupsAndPersists() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    _ = try await store.addWatchedFolder("/tmp/models-a")
    _ = try await store.addWatchedFolder("/tmp/models-a")
    let settings = try await store.addWatchedFolder("~/models-b")
    #expect(settings.watchedFolders.count == 2)
    #expect(settings.watchedFolders[1].hasSuffix("/models-b"))
    #expect(!settings.watchedFolders[1].contains("~"))

    let reloaded = await SettingsStore(directory: dir).models()
    #expect(reloaded == settings)

    let afterRemove = try await store.removeWatchedFolder("/tmp/models-a")
    #expect(afterRemove.watchedFolders.count == 1)
    let reloadedAgain = await SettingsStore(directory: dir).models()
    #expect(reloadedAgain.watchedFolders == afterRemove.watchedFolders)
}

@Test func watchedFolderFlowsIntoDiscovery() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let modelsDir = dir.appendingPathComponent("my-models")
    try DiscoveryFixtures.makeGGUF(
        at: modelsDir.appendingPathComponent("hidden-model.gguf"), bytes: 2048)

    let kernelDir = dir.appendingPathComponent("appsupport")
    let kernel = Kernel(directory: kernelDir, adapters: [])
    try await kernel.addWatchedFolder(modelsDir.path)
    #expect(try await kernel.watchedFolders() == [modelsDir.path])

    let scanner = LooseFileScanner(
        directories: (try await kernel.watchedFolders()).map {
            URL(fileURLWithPath: $0, isDirectory: true)
        })
    let result = await scanner.scan()
    #expect(result.discovered.contains { $0.name == "hidden-model" })

    try await kernel.removeWatchedFolder(modelsDir.path)
    #expect(try await kernel.watchedFolders().isEmpty)
}

@Test func updatingModelsSettingsDrivesGovernorResidencyPolicy() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let governor = MemoryGovernor(
        totalMemoryMB: 65536, heavyThresholdMB: 1024, defaultWarmWindow: .seconds(300))
    let kernel = Kernel(directory: dir, adapters: [], governor: governor)

    var models = await kernel.modelsSettings()
    models.keepWarm = .never
    models.eviction = .budgeted
    models.ramBudgetMB = 20000
    try await kernel.updateModelsSettings(models)

    #expect(await governor.residency.warmWindow(for: "any-model") == .zero)
    #expect(await governor.currentEvictionPolicy() == .budgeted)

    let unloaded = CleanupFlag()
    await governor.markLoaded(modelID: "llm", name: "llm", footprintMB: 4000) {
        unloaded.mark()
    }
    await governor.beginGeneration("llm")
    await governor.endGeneration("llm")
    try await waitUntil { await governor.isResident("llm") == false }
    #expect(unloaded.wasInvoked)
}

@Test func budgetedEvictionKeepsResidentsWithinBudgetAndEvictsOldestOverIt() async throws {
    let governor = MemoryGovernor(
        totalMemoryMB: 65536, heavyThresholdMB: 1024, defaultWarmWindow: .seconds(300))
    await governor.apply(
        policy: ResidencyPolicy(keepWarm: .oneHour, eviction: .budgeted, ramBudgetMB: 16000))

    let firstUnloaded = CleanupFlag()
    await governor.markLoaded(modelID: "first", name: "first", footprintMB: 6000) {
        firstUnloaded.mark()
    }
    try await governor.admit(modelID: "second", name: "second", footprintMB: 6000)
    await governor.markLoaded(modelID: "second", name: "second", footprintMB: 6000) {}
    #expect(firstUnloaded.wasInvoked == false)
    #expect(await governor.isResident("first"))
    #expect(await governor.isResident("second"))

    try await governor.admit(modelID: "third", name: "third", footprintMB: 6000)
    #expect(firstUnloaded.wasInvoked)
    #expect(await governor.isResident("first") == false)
    #expect(await governor.isResident("second"))
    #expect(await governor.isResident("third"))
}

@Test func advancedSettingsDriveJobHistoryLimit() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    var advanced = await kernel.advancedSettings()
    advanced.jobHistoryLimit = 2
    try await kernel.updateAdvancedSettings(advanced)

    #expect(await kernel.scheduler.history.limit == 2)
}

@Test func appearanceFontPairIsSparseAndLenient() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let settingsDir = dir.appendingPathComponent("settings")
    try FileManager.default.createDirectory(at: settingsDir, withIntermediateDirectories: true)
    try """
        {"theme": "dark", "uiFont": 42, "monoFont": ["Menlo"]}
        """.write(
        to: settingsDir.appendingPathComponent("appearance.json"), atomically: true,
        encoding: .utf8)

    let store = SettingsStore(directory: dir)
    let appearance = await store.appearance()
    #expect(appearance.theme == .dark)
    #expect(appearance.uiFont == nil)
    #expect(appearance.monoFont == nil)

    var chosen = appearance
    chosen.uiFont = "Charter"
    try await store.save(chosen)
    let reloaded = await SettingsStore(directory: dir).appearance()
    #expect(reloaded.uiFont == "Charter")
    #expect(reloaded.monoFont == nil)
}

@Test func chatAndGeneralSettingsCarryTheirNewFields() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    var chat = await store.chat()
    #expect(chat.showStats)
    #expect(chat.sendWithEnter)
    #expect(chat.exportFormat == .markdown)
    chat.showStats = false
    chat.sendWithEnter = false
    chat.exportFormat = .json
    try await store.save(chat)

    var general = await store.general()
    #expect(general.fixedMode == nil)
    #expect(general.quickAskHotkey == nil)
    #expect(general.menuBarItem == false)
    general.restoreLastSession = false
    general.fixedMode = AppMode.images
    general.quickAskHotkey = QuickAskHotkey(keyCode: 49, modifiers: 768)
    general.menuBarItem = true
    try await store.save(general)

    let reloaded = SettingsStore(directory: dir)
    let chatBack = await reloaded.chat()
    #expect(chatBack.showStats == false)
    #expect(chatBack.sendWithEnter == false)
    #expect(chatBack.exportFormat == .json)
    let generalBack = await reloaded.general()
    #expect(generalBack.restoreLastSession == false)
    #expect(generalBack.fixedMode == AppMode.images)
    #expect(generalBack.quickAskHotkey == QuickAskHotkey(keyCode: 49, modifiers: 768))
    #expect(generalBack.menuBarItem == true)
}

@Test func residentModelsSurfaceThroughTheKernel() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let governor = MemoryGovernor(
        totalMemoryMB: 65536, heavyThresholdMB: 1024, defaultWarmWindow: .seconds(300))
    let kernel = Kernel(directory: dir, adapters: [], governor: governor)

    #expect(await kernel.residentModels().isEmpty)
    await governor.markLoaded(modelID: "llm", name: "llm", footprintMB: 4000) {}
    let resident = await kernel.residentModels()
    #expect(resident.map(\.modelID) == ["llm"])
    #expect(resident.map(\.origin) == [.governor])
}

@Test func ollamaLoadedModelsParseTheApiPsPayload() {
    let payload = """
        {"models":[{"name":"gemma4:latest","model":"gemma4:latest",\
        "size":10473229515,"digest":"abc"},{"name":"qwen3.5:latest",\
        "model":"qwen3.5:latest","size":5242880000}]}
        """
    let parsed = OllamaAdapter.parseLoadedModels(Data(payload.utf8))
    #expect(parsed.map(\.name) == ["gemma4:latest", "qwen3.5:latest"])
    #expect(parsed[0].sizeMB == 9988)
    #expect(OllamaAdapter.parseLoadedModels(Data("nonsense".utf8)).isEmpty)
}

@Test func governorExposesItsDefaultBudget() async {
    let governor = MemoryGovernor(
        totalMemoryMB: 32768, heavyThresholdMB: 1024, tightFraction: 0.8,
        defaultWarmWindow: .seconds(300))
    #expect(await governor.defaultBudgetMB == 26214)
}

@Test func hfCacheRootsRoundTripAndDecodeLeniently() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    _ = try await store.addHFCacheRoot("~/models/huggingface")
    _ = try await store.addHFCacheRoot("~/models/huggingface")
    let home = FileManager.default.homeDirectoryForCurrentUser.path
    #expect(await store.models().hfCacheRoots == ["\(home)/models/huggingface"])

    let reloaded = SettingsStore(directory: dir)
    #expect(await reloaded.models().hfCacheRoots == ["\(home)/models/huggingface"])

    _ = try await reloaded.removeHFCacheRoot("~/models/huggingface")
    #expect(await reloaded.models().hfCacheRoots.isEmpty)

    let legacy = dir.appendingPathComponent("settings/models.json")
    try Data(#"{"watchedFolders": ["/tmp/x"]}"#.utf8).write(to: legacy)
    let lenient = SettingsStore(directory: dir)
    #expect(await lenient.models().hfCacheRoots.isEmpty)
    #expect(await lenient.models().watchedFolders == ["/tmp/x"])
}
