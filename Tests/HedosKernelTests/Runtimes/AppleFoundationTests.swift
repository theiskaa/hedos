import Foundation
import Testing

@testable import HedosKernel

private final class RecordedInvocation: @unchecked Sendable {
    private let lock = NSLock()
    private var _messages: [ChatMessage] = []
    private var _temperature: Double?
    private var _maxTokens: Int?
    private var _terminated = false

    func record(messages: [ChatMessage], temperature: Double?, maxTokens: Int?) {
        lock.lock()
        _messages = messages
        _temperature = temperature
        _maxTokens = maxTokens
        lock.unlock()
    }

    func markTerminated() {
        lock.lock()
        _terminated = true
        lock.unlock()
    }

    var messages: [ChatMessage] {
        lock.lock()
        defer { lock.unlock() }
        return _messages
    }

    var temperature: Double? {
        lock.lock()
        defer { lock.unlock() }
        return _temperature
    }

    var maxTokens: Int? {
        lock.lock()
        defer { lock.unlock() }
        return _maxTokens
    }

    var terminated: Bool {
        lock.lock()
        defer { lock.unlock() }
        return _terminated
    }
}

private struct FakeFoundationBackend: AppleFoundationBackend {
    var state: BuiltinAvailability = .available
    var events: [BuiltinGenerationEvent] = []
    var failure: KernelError?
    var delayNs: UInt64 = 0
    var recorded = RecordedInvocation()

    func availability() -> BuiltinAvailability {
        state
    }

    func stream(
        messages: [ChatMessage], temperature: Double?, maxTokens: Int?
    ) -> AsyncThrowingStream<BuiltinGenerationEvent, Error> {
        recorded.record(messages: messages, temperature: temperature, maxTokens: maxTokens)
        let events = events
        let failure = failure
        let delayNs = delayNs
        let recorded = recorded
        return AsyncThrowingStream { continuation in
            let task = Task {
                for event in events {
                    if delayNs > 0 { try? await Task.sleep(nanoseconds: delayNs) }
                    if Task.isCancelled { break }
                    continuation.yield(event)
                }
                if let failure {
                    continuation.finish(throwing: failure)
                } else {
                    continuation.finish()
                }
            }
            continuation.onTermination = { _ in
                task.cancel()
                recorded.markTerminated()
            }
        }
    }
}

private func builtinRecord() -> ModelRecord {
    ModelRecord(
        name: "Apple Intelligence",
        modality: .text,
        capabilities: [.chat, .complete],
        source: ModelSource(kind: .builtin, path: AppleFoundationScanner.sourcePath),
        runtime: RuntimeRef(id: "apple-foundation", resolved: .auto, tier: .native),
        execution: .stream,
        footprintMB: 0,
        state: .ready,
        registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
}

private struct FakeOllamaScanner: StoreScanner {
    var kinds: Set<SourceKind> { [.ollama] }

    func scan() async -> ScanResult {
        ScanResult(discovered: [
            DiscoveredModel(
                name: "qwen-fake",
                source: ModelSource(kind: .ollama, path: "ollama://qwen-fake"),
                modalityHint: .text,
                capabilitiesHint: [.chat, .complete],
                executionHint: .stream,
                footprintBytes: 5_000_000_000)
        ])
    }
}

@Test func scannerEmitsWeightlessRecordWhenAvailable() async {
    let scanner = AppleFoundationScanner(backend: FakeFoundationBackend(state: .available))
    let result = await scanner.scan()
    #expect(result.issues.isEmpty)
    let model = result.discovered.first
    #expect(result.discovered.count == 1)
    #expect(model?.name == "Apple Intelligence")
    #expect(model?.source.kind == .builtin)
    #expect(model?.source.path == AppleFoundationScanner.sourcePath)
    #expect(model?.footprintBytes == 0)
    #expect(model?.modalityHint == .text)
    #expect(model?.capabilitiesHint == [.chat, .complete])
    #expect(model?.executionHint == .stream)
}

@Test func scannerWithholdsRecordAndNamesTheReason() async {
    let notEnabled = await AppleFoundationScanner(
        backend: FakeFoundationBackend(state: .notEnabled)
    ).scan()
    #expect(notEnabled.discovered.isEmpty)
    #expect(notEnabled.issues.count == 1)
    #expect(notEnabled.issues.first?.contains("System Settings") == true)

    let notReady = await AppleFoundationScanner(
        backend: FakeFoundationBackend(state: .notReady)
    ).scan()
    #expect(notReady.discovered.isEmpty)
    #expect(notReady.issues.first?.contains("downloading") == true)

    let notEligible = await AppleFoundationScanner(
        backend: FakeFoundationBackend(state: .notEligible)
    ).scan()
    #expect(notEligible.discovered.isEmpty)
    #expect(notEligible.issues.isEmpty)
}

@Test func availabilityRoundTripsThroughDiscoveryAsMissingThenHealed() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)

    _ = try await DiscoveryService(scanners: [
        AppleFoundationScanner(backend: FakeFoundationBackend(state: .available))
    ]).discover(into: registry)
    let registered = try #require(try await registry.list().first)
    #expect(registered.name == "Apple Intelligence")
    #expect(registered.state == .unresolved)

    _ = try await DiscoveryService(scanners: [
        AppleFoundationScanner(backend: FakeFoundationBackend(state: .notEnabled))
    ]).discover(into: registry)
    let missing = try #require(try await registry.get(id: registered.id))
    #expect(missing.state == .missing)

    _ = try await DiscoveryService(scanners: [
        AppleFoundationScanner(backend: FakeFoundationBackend(state: .available))
    ]).discover(into: registry)
    let healed = try #require(try await registry.get(id: registered.id))
    #expect(healed.state == .unresolved)
}

@Test func builtinRecordIdentifiesWithHonestParams() {
    let identified = Identification.identify(builtinRecord())
    #expect(identified.format == .builtin)
    #expect(identified.modality == .text)
    #expect(identified.capabilities == [.chat, .complete])
    #expect(identified.execution == .stream)
    #expect(identified.params.map(\.key) == ["temperature", "max_tokens"])
}

@Test func resolutionPutsAppleFoundationNativeReadyWithoutProfileNoise() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    var record = builtinRecord()
    record.runtime = RuntimeRef()
    record.state = .unresolved
    try await registry.register(record)

    let engine = ResolutionEngine(adapters: [
        AppleFoundationAdapter(backend: FakeFoundationBackend())
    ])
    try await engine.resolveAll(in: registry)

    let resolved = try #require(try await registry.get(id: record.id))
    #expect(resolved.runtime.id == "apple-foundation")
    #expect(resolved.runtime.tier == .native)
    #expect(resolved.state == .ready)
    let keys = resolved.params.map(\.key)
    #expect(!keys.contains("top_p"))
    #expect(!keys.contains("context_length"))
    #expect(!keys.contains("thinking"))
}

@Test func adapterDiffsCumulativeSnapshotsIntoDeltas() async throws {
    let backend = FakeFoundationBackend(events: [
        .snapshot("Hel"), .snapshot("Hello"), .snapshot("Hello wor"), .snapshot("Hello world"),
        .done(promptTokens: nil, completionTokens: nil),
    ])
    let adapter = AppleFoundationAdapter(backend: backend)
    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])

    var deltas: [String] = []
    var sawDone = false
    for try await chunk in adapter.invoke(builtinRecord(), .chat, payload: payload) {
        switch chunk {
        case .text(let delta): deltas.append(delta)
        case .done: sawDone = true
        default: break
        }
    }
    #expect(deltas == ["Hel", "lo", " wor", "ld"])
    #expect(sawDone)
}

@Test func deltaTreatsNonPrefixSnapshotAsRestart() {
    #expect(AppleFoundationAdapter.delta(previous: "abc", current: "xy") == "xy")
    #expect(AppleFoundationAdapter.delta(previous: "ab", current: "abcd") == "cd")
    #expect(AppleFoundationAdapter.delta(previous: "same", current: "same") == "")
}

@Test func adapterReportsStatsFromBackendAndClock() async throws {
    let backend = FakeFoundationBackend(events: [
        .snapshot("hello"), .done(promptTokens: 12, completionTokens: 34),
    ])
    let adapter = AppleFoundationAdapter(backend: backend)
    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])

    var stats: GenerationStats?
    for try await chunk in adapter.invoke(builtinRecord(), .chat, payload: payload) {
        if case .done(let s) = chunk { stats = s }
    }
    #expect(stats?.promptTokens == 12)
    #expect(stats?.completionTokens == 34)
    #expect(stats?.durationMs != nil)
    #expect(stats?.ttftMs != nil)
}

@Test func adapterForwardsTemperatureAndMaxTokensAndOmitsWhenUnset() async throws {
    let recorded = RecordedInvocation()
    var backend = FakeFoundationBackend(events: [.done(promptTokens: nil, completionTokens: nil)])
    backend.recorded = recorded
    let adapter = AppleFoundationAdapter(backend: backend)
    let tuned: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])]),
        "temperature": .double(0.4),
        "max_tokens": .int(128),
    ])
    for try await _ in adapter.invoke(builtinRecord(), .chat, payload: tuned) {}
    #expect(recorded.temperature == 0.4)
    #expect(recorded.maxTokens == 128)

    let bareRecorded = RecordedInvocation()
    var bareBackend = FakeFoundationBackend(events: [
        .done(promptTokens: nil, completionTokens: nil)
    ])
    bareBackend.recorded = bareRecorded
    let bareAdapter = AppleFoundationAdapter(backend: bareBackend)
    let bare: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])
    for try await _ in bareAdapter.invoke(builtinRecord(), .chat, payload: bare) {}
    #expect(bareRecorded.temperature == nil)
    #expect(bareRecorded.maxTokens == nil)
}

@Test func completeWrapsPromptAsSingleUserMessage() async throws {
    let recorded = RecordedInvocation()
    var backend = FakeFoundationBackend(events: [.done(promptTokens: nil, completionTokens: nil)])
    backend.recorded = recorded
    let adapter = AppleFoundationAdapter(backend: backend)
    for try await _ in adapter.invoke(
        builtinRecord(), .complete, payload: .object(["prompt": .string("2+2?")])) {}
    #expect(recorded.messages == [ChatMessage(role: .user, content: "2+2?")])
}

@Test func adapterSurfacesBackendErrorsAfterPartialText() async {
    let backend = FakeFoundationBackend(
        events: [.snapshot("Hel")],
        failure: KernelError.runtimeFailed("Apple's model declined this request."))
    let adapter = AppleFoundationAdapter(backend: backend)
    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("hi")])])
    ])

    var deltas: [String] = []
    do {
        for try await chunk in adapter.invoke(builtinRecord(), .chat, payload: payload) {
            if case .text(let delta) = chunk { deltas.append(delta) }
        }
        Issue.record("expected the backend error to surface")
    } catch {
        #expect(String(describing: error).contains("model declined this request"))
    }
    #expect(deltas == ["Hel"])
}

@Test func adapterCancelsBackendWhenConsumerStops() async throws {
    let recorded = RecordedInvocation()
    var backend = FakeFoundationBackend(
        events: (1...200).map { .snapshot(String(repeating: "x", count: $0)) },
        delayNs: 20_000_000)
    backend.recorded = recorded
    let adapter = AppleFoundationAdapter(backend: backend)
    let payload: JSONValue = .object([
        "messages": .array([.object(["role": .string("user"), "content": .string("go")])])
    ])

    let consumer = Task {
        for try await chunk in adapter.invoke(builtinRecord(), .chat, payload: payload) {
            if case .text = chunk { break }
        }
    }
    _ = await consumer.result

    var cancelled = false
    for _ in 0..<40 {
        try await Task.sleep(for: .milliseconds(50))
        if recorded.terminated {
            cancelled = true
            break
        }
    }
    #expect(cancelled)
}

@Test func kernelChatsEndToEndThroughAppleFoundation() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let backend = FakeFoundationBackend(events: [
        .snapshot("Hi"), .snapshot("Hi there"), .done(promptTokens: 2, completionTokens: 2),
    ])
    let kernel = Kernel(directory: dir, adapters: [AppleFoundationAdapter(backend: backend)])
    let record = builtinRecord()
    try await kernel.registry.register(record)

    var deltas: [String] = []
    var stats: GenerationStats?
    let stream = try await kernel.chat(record.id, messages: [.init(role: .user, content: "yo")])
    for try await chunk in stream {
        switch chunk {
        case .text(let delta): deltas.append(delta)
        case .done(let s): stats = s
        default: break
        }
    }
    #expect(deltas == ["Hi", " there"])
    #expect(stats?.completionTokens == 2)
}

@Test func splitSeparatesInstructionsHistoryAndPrompt() {
    let full = SystemFoundationBackend.split([
        ChatMessage(role: .system, content: "be brief"),
        ChatMessage(role: .user, content: "one"),
        ChatMessage(role: .assistant, content: "two"),
        ChatMessage(role: .user, content: "three"),
    ])
    #expect(full.instructions == "be brief")
    #expect(full.history == [
        ChatMessage(role: .user, content: "one"),
        ChatMessage(role: .assistant, content: "two"),
    ])
    #expect(full.prompt == "three")

    let bare = SystemFoundationBackend.split([ChatMessage(role: .user, content: "solo")])
    #expect(bare.instructions == nil)
    #expect(bare.history.isEmpty)
    #expect(bare.prompt == "solo")
}

@Test func headlineCountsTheBuiltinModelWithoutDistortingBytes() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let registry = Registry(directory: dir)
    let summary = try await DiscoveryService(scanners: [
        FakeOllamaScanner(),
        AppleFoundationScanner(backend: FakeFoundationBackend(state: .available)),
    ]).discover(into: registry)

    #expect(summary.totalCount == 2)
    #expect(summary.headline.contains("Found 2 models"))
    #expect(summary.headline.contains("1 in Ollama, 1 built in"))
    #expect(summary.perKind[.builtin]?.bytes == 0)
    #expect(summary.totalBytes == 5_000_000_000)
}
