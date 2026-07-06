import Foundation
import Testing

@testable import HedosKernel

@Test func modeOrderMatchesCommandShortcuts() {
    #expect(AppMode.allCases == [.chat, .images, .voice, .library, .settings])
    #expect(AppMode.chat.ordinal == 1)
    #expect(AppMode.images.ordinal == 2)
    #expect(AppMode.voice.ordinal == 3)
    #expect(AppMode.library.ordinal == 4)
    #expect(AppMode.settings.ordinal == 5)
    for mode in AppMode.allCases {
        #expect(AppMode.at(ordinal: mode.ordinal) == mode)
    }
    #expect(AppMode.at(ordinal: 0) == nil)
    #expect(AppMode.at(ordinal: 6) == nil)
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

@Test func shellStateSelectionAccessorsCoverEveryMode() {
    var state = ShellState()
    #expect(state.mode == .library)
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

    #expect(try await store.shellState() == ShellState())

    var state = ShellState(mode: .images)
    state.setSelection("model:abcd1234", in: .images)
    state.setSelection("session-1", in: .chat)
    try await store.saveShellState(state)

    let relaunched = try await SettingsStore(directory: dir).shellState()
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

    let reloaded = try await SettingsStore(directory: dir).load()
    #expect(reloaded.watchedFolders == ["/tmp/models-a", "/tmp/models-b"])
    #expect(reloaded.shell == ShellState(mode: .voice, voiceModelID: "kokoro"))
}

@Test func legacySettingsFileWithoutShellDecodesToDefaults() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let legacy = """
        {"schemaVersion": 1, "watchedFolders": ["/tmp/old-models"]}
        """
    try Data(legacy.utf8).write(to: dir.appendingPathComponent("settings.json"))

    let settings = try await SettingsStore(directory: dir).load()
    #expect(settings.watchedFolders == ["/tmp/old-models"])
    #expect(settings.shell == ShellState())
}

@Test func unknownShellModeFallsBackToLibrary() async throws {
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

    let state = try await SettingsStore(directory: dir).shellState()
    #expect(state.mode == .library)
    #expect(state.chatSessionID == "s-1")
}

@Test func kernelExposesShellStateRoundTrip() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    #expect(try await kernel.shellState() == ShellState())
    let state = ShellState(mode: .chat, chatSessionID: "session-9")
    try await kernel.saveShellState(state)
    #expect(try await kernel.shellState() == state)

    let relaunched = Kernel(directory: dir, adapters: [])
    #expect(try await relaunched.shellState() == state)
}
