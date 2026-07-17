import Foundation
import Testing

@testable import HedosKernel

@Test func modeCaseOrderIsStable() {
    #expect(
        AppMode.allCases == [
            .home, .chat, .images, .voice, .library, .gateway, .settings,
        ])
    #expect(AppMode(rawValue: "home") == .home)
}

@Test func launcherRoutesByCapabilityNotName() {
    var chatModel = Fixtures.gguf()
    chatModel.runtime = RuntimeRef(id: "llama.cpp", resolved: .auto, tier: .native)
    chatModel.state = .ready
    #expect(Launcher.destination(for: chatModel) == .chat)

    #expect(Launcher.destination(for: Fixtures.flux()) == .images)

    var speaker = Fixtures.gguf(path: "~/Downloads/reader.gguf")
    speaker.capabilities = [.speak]
    speaker.runtime = RuntimeRef(id: "python:mlx-audio", resolved: .auto, tier: .managed)
    speaker.state = .ready
    #expect(Launcher.destination(for: speaker) == .voice)

    let unresolved = Fixtures.gguf()
    #expect(Launcher.destination(for: unresolved) == .library)

    var recipeNeeded = Fixtures.flux()
    recipeNeeded.runtime = RuntimeRef(
        id: "python:mflux", resolved: .auto, tier: .recipeNeeded)
    #expect(Launcher.destination(for: recipeNeeded) == .library)

    var embedder = Fixtures.gguf(path: "~/Downloads/embedder.gguf")
    embedder.capabilities = [.embed]
    embedder.runtime = RuntimeRef(id: "llama.cpp", resolved: .auto, tier: .native)
    #expect(Launcher.destination(for: embedder) == .library)
}

@Test func launcherPicksDefaultChatModelAndFiltersShelfPerMode() {
    var chatModel = Fixtures.gguf()
    chatModel.runtime = RuntimeRef(id: "llama.cpp", resolved: .auto, tier: .native)
    chatModel.state = .ready
    var missingChatModel = Fixtures.gguf(path: "~/Downloads/gone.gguf")
    missingChatModel.runtime = RuntimeRef(id: "llama.cpp", resolved: .auto, tier: .native)
    missingChatModel.state = .missing
    var speaker = Fixtures.gguf(path: "~/Downloads/reader.gguf")
    speaker.capabilities = [.speak]
    speaker.runtime = RuntimeRef(id: "python:mlx-audio", resolved: .auto, tier: .managed)
    speaker.state = .ready
    let shelf = [missingChatModel, Fixtures.flux(), speaker, chatModel]

    #expect(Launcher.defaultChatModel(in: shelf)?.id == chatModel.id)
    #expect(Launcher.models(in: shelf, for: .chat).map(\.id) == [missingChatModel.id, chatModel.id])
    #expect(Launcher.models(in: shelf, for: .images).map(\.id) == [Fixtures.flux().id])
    #expect(Launcher.models(in: shelf, for: .voice).map(\.id) == [speaker.id])
    #expect(Launcher.defaultChatModel(in: [Fixtures.flux(), speaker]) == nil)
}

@Test func modeSidebarsIncludeNonRunnableModelsOfTheirModality() {
    var recipeImage = Fixtures.gguf(path: "~/models/sdxl-turbo")
    recipeImage.modality = .image
    recipeImage.capabilities = [.image]
    recipeImage.runtime = RuntimeRef(id: nil, resolved: .unresolved, tier: .recipeNeeded)
    recipeImage.state = .unresolved
    var recipeVoice = Fixtures.gguf(path: "~/models/kokoro-alt")
    recipeVoice.modality = .speech
    recipeVoice.capabilities = [.speak]
    recipeVoice.runtime = .unresolved
    recipeVoice.state = .unresolved
    var mystery = Fixtures.gguf(path: "~/models/mystery")
    mystery.modality = .unknown
    mystery.capabilities = []
    mystery.runtime = .unresolved
    mystery.state = .unresolved
    let readyImage = Fixtures.flux()
    let shelf = [recipeImage, recipeVoice, mystery, readyImage]

    #expect(Launcher.destination(for: recipeImage) == .library)
    #expect(
        Launcher.models(in: shelf, for: .images).map(\.id) == [recipeImage.id, readyImage.id])
    #expect(Launcher.models(in: shelf, for: .voice).map(\.id) == [recipeVoice.id])
    #expect(Launcher.models(in: shelf, for: .chat).isEmpty)
    #expect(!Launcher.models(in: shelf, for: .images).contains { $0.id == mystery.id })
    #expect(!Launcher.models(in: shelf, for: .voice).contains { $0.id == mystery.id })
}

@Test func shellStateSelectionAccessorsCoverEveryMode() {
    var state = ShellState()
    #expect(state.mode == .home)
    for mode in AppMode.allCases {
        state.setSelection("id-\(mode.rawValue)", in: mode)
    }
    #expect(state.selection(in: .chat) == "id-chat")
    #expect(state.selection(in: .images) == "id-images")
    #expect(state.selection(in: .voice) == "id-voice")
    #expect(state.selection(in: .library) == "id-library")
    #expect(state.selection(in: .settings) == nil)
}

@Test func shellStateRestoresAcrossRelaunch() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    #expect(await store.shellState() == ShellState())

    var state = ShellState(mode: .images)
    state.setSelection("model:abcd1234", in: .images)
    state.setSelection("session-1", in: .chat)
    try await store.saveShellState(state)

    let relaunched = await SettingsStore(directory: dir).shellState()
    #expect(relaunched == state)
    #expect(relaunched.mode == .images)
    #expect(relaunched.selection(in: .images) == "model:abcd1234")
    #expect(relaunched.selection(in: .chat) == "session-1")
}

@Test func shellStateCoexistsWithWatchedFolders() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    _ = try await store.addWatchedFolder("/tmp/models-a")
    try await store.saveShellState(ShellState(mode: .voice, voiceModelID: "kokoro"))
    _ = try await store.addWatchedFolder("/tmp/models-b")

    let reloaded = SettingsStore(directory: dir)
    #expect(await reloaded.models().watchedFolders == ["/tmp/models-a", "/tmp/models-b"])
    #expect(await reloaded.shellState() == ShellState(mode: .voice, voiceModelID: "kokoro"))
}

@Test func legacySettingsFileWithoutShellDecodesToDefaults() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let legacy = """
        {"schemaVersion": 1, "watchedFolders": ["/tmp/old-models"]}
        """
    try Data(legacy.utf8).write(to: dir.appendingPathComponent("settings.json"))

    let store = SettingsStore(directory: dir)
    #expect(await store.models().watchedFolders == ["/tmp/old-models"])
    #expect(await store.shellState() == ShellState())
}

@Test func sidebarCollapsedPersistsAndDefaultsToFalse() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = SettingsStore(directory: dir)

    #expect(await store.shellState().sidebarCollapsed == false)

    var state = ShellState(mode: .chat)
    state.sidebarCollapsed = true
    try await store.saveShellState(state)
    #expect(await SettingsStore(directory: dir).shellState().sidebarCollapsed == true)

    let legacy = """
        {"shell": {"mode": "voice"}}
        """
    let fresh = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: fresh) }
    try FileManager.default.createDirectory(
        at: fresh.appendingPathComponent("settings"), withIntermediateDirectories: true)
    try Data(legacy.utf8).write(
        to: fresh.appendingPathComponent("settings/shell.json"))
    let decoded = await SettingsStore(directory: fresh).shellState()
    #expect(decoded.mode == .voice)
    #expect(decoded.sidebarCollapsed == false)
}

@Test func unknownShellModeFallsBackToHome() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let future = """
        {
            "schemaVersion": 1,
            "watchedFolders": [],
            "shell": {"mode": "holodeck", "chatSessionID": "s-1"}
        }
        """
    try Data(future.utf8).write(to: dir.appendingPathComponent("settings.json"))

    let state = await SettingsStore(directory: dir).shellState()
    #expect(state.mode == .home)
    #expect(state.chatSessionID == "s-1")
}

@Test func kernelExposesShellStateRoundTrip() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    #expect(await kernel.settings.shellState() == ShellState())
    let state = ShellState(mode: .chat, chatSessionID: "session-9")
    try await kernel.settings.saveShellState(state)
    #expect(await kernel.settings.shellState() == state)

    let relaunched = Kernel(directory: dir, adapters: [])
    #expect(await relaunched.settings.shellState() == state)
}
