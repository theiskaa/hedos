import Foundation
import Testing

@testable import HedosKernel

private func waitUntil(
    _ condition: @Sendable () async -> Bool
) async throws {
    for _ in 0..<600 {
        if await condition() { return }
        try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("condition never became true")
}

private actor EventLog {
    private(set) var events: [VoiceLoop.Event] = []

    func record(_ event: VoiceLoop.Event) {
        events.append(event)
    }

    func userTurns() -> [String] {
        events.compactMap {
            if case .userTurn(let text) = $0 { return text }
            return nil
        }
    }

    func completions() -> Int {
        events.filter {
            if case .turnCompleted = $0 { return true }
            return false
        }.count
    }

    func bargeIns() -> Int {
        events.filter {
            if case .userSpeechBegan = $0 { return true }
            return false
        }.count
    }

    func speechFrames() -> Int {
        events.filter {
            if case .speech = $0 { return true }
            return false
        }.count
    }

    func assistantText() -> String {
        events.compactMap {
            if case .assistantDelta(let delta) = $0 { return delta }
            return nil
        }.joined()
    }
}

private actor SpokenLog {
    private(set) var sentences: [String] = []
    var stallFirstCall = false
    private var calls = 0

    func setStallFirstCall() {
        stallFirstCall = true
    }

    func note(_ sentence: String) -> Int {
        sentences.append(sentence)
        calls += 1
        return calls
    }
}

private func textStream(_ parts: [String]) -> AsyncThrowingStream<CapabilityChunk, Error> {
    AsyncThrowingStream { continuation in
        for part in parts {
            continuation.yield(.text(part))
        }
        continuation.yield(.done(nil))
        continuation.finish()
    }
}

private let tinyVAD = VADLite.Config(
    sampleRate: 16000, hopSamples: 4, speechRMS: 0.05,
    startHops: 2, endSilenceHops: 3, minTurnHops: 2, preRollHops: 2)

private let voiced = [Float](repeating: 0.2, count: 4)
private let quiet = [Float](repeating: 0.0, count: 4)

@Test func vadDetectsTurnsWithPreRollAndDiscardsBlips() {
    var vad = VADLite(config: tinyVAD)
    #expect(vad.consume(quiet + quiet + quiet).isEmpty)

    var events = vad.consume(voiced)
    #expect(events.isEmpty)
    events = vad.consume(voiced)
    #expect(events == [.speechStarted])
    events = vad.consume(voiced + quiet + quiet + quiet)
    guard case .turnEnded(let samples)? = events.first else {
        Issue.record("expected turnEnded, got \(events)")
        return
    }
    #expect(samples.count == 24)

    var blippy = VADLite(
        config: VADLite.Config(
            sampleRate: 16000, hopSamples: 4, speechRMS: 0.05,
            startHops: 1, endSilenceHops: 2, minTurnHops: 4, preRollHops: 1))
    #expect(blippy.consume(voiced) == [.speechStarted])
    #expect(blippy.consume(quiet + quiet).isEmpty)
    #expect(blippy.consume(voiced) == [.speechStarted])
}

@Test func sentenceChunkerSplitsOnBoundariesAndRespectsMinLength() {
    var chunker = SentenceChunker()
    #expect(chunker.consume("First sentence here. And the") == ["First sentence here."])
    #expect(chunker.consume(" rest arrives!").isEmpty == false)
    #expect(chunker.flush() == nil)

    var short = SentenceChunker()
    #expect(short.consume("Hi. There friend. ") == ["Hi. There friend."])

    var lines = SentenceChunker()
    #expect(lines.consume("One\nTwo") == ["One"])
    #expect(lines.flush() == "Two")

    var trailing = SentenceChunker()
    #expect(trailing.consume("No boundary yet").isEmpty)
    #expect(trailing.flush() == "No boundary yet")
}

@Test func voiceLoopComposesTranscribeChatSpeakAndPersistsOrder() async throws {
    let spoken = SpokenLog()
    let backends = VoiceLoopBackends(
        transcribe: { _ in textStream(["what is", " up"]) },
        chat: { question in
            #expect(question == "what is up")
            return textStream(["First sentence here. ", "Second bit follows."])
        },
        speak: { sentence in
            AsyncThrowingStream { continuation in
                Task {
                    _ = await spoken.note(sentence)
                    continuation.yield(
                        .audio(AudioFrame(data: Data([1, 2, 3, 4]), sampleRate: 24000)))
                    continuation.yield(.done(nil))
                    continuation.finish()
                }
            }
        })

    let loop = VoiceLoop(backends: backends, vadConfig: tinyVAD)
    let log = EventLog()
    let events = await loop.start()
    let collector = Task {
        for await event in events {
            await log.record(event)
        }
    }

    await loop.feed(voiced + voiced + voiced)
    await loop.feed(quiet + quiet + quiet)
    try await waitUntil { await log.completions() == 1 }
    await loop.stop()
    _ = await collector.value

    #expect(await log.userTurns() == ["what is up"])
    #expect(await log.assistantText() == "First sentence here. Second bit follows.")
    #expect(await log.speechFrames() == 2)
    #expect(await spoken.sentences == ["First sentence here.", "Second bit follows."])
}

private actor PersistLog {
    var calls: [(pcm: Data, sampleRate: Int, text: String)] = []
    func record(_ pcm: Data, _ sampleRate: Int, _ text: String) {
        calls.append((pcm, sampleRate, text))
    }
    var count: Int { calls.count }
    var last: (pcm: Data, sampleRate: Int, text: String)? { calls.last }
}

@Test func completedVoiceTurnHandsAccumulatedAudioAndTextToPersistClosure() async throws {
    let persisted = PersistLog()
    let backends = VoiceLoopBackends(
        transcribe: { _ in textStream(["hello"]) },
        chat: { _ in textStream(["The reply here."]) },
        speak: { _ in
            AsyncThrowingStream { continuation in
                Task {
                    continuation.yield(
                        .audio(AudioFrame(data: Data([1, 2]), sampleRate: 22050)))
                    continuation.yield(
                        .audio(AudioFrame(data: Data([3, 4]), sampleRate: 22050)))
                    continuation.yield(.done(nil))
                    continuation.finish()
                }
            }
        },
        persistTurnAudio: { pcm, sampleRate, text in
            await persisted.record(pcm, sampleRate, text)
        })

    let loop = VoiceLoop(backends: backends, vadConfig: tinyVAD)
    let log = EventLog()
    let events = await loop.start()
    let collector = Task {
        for await event in events { await log.record(event) }
    }
    await loop.feed(voiced + voiced + voiced)
    await loop.feed(quiet + quiet + quiet)
    try await waitUntil { await log.completions() == 1 }
    await loop.stop()
    _ = await collector.value

    #expect(await persisted.count == 1)
    let call = await persisted.last
    #expect(call?.pcm == Data([1, 2, 3, 4]))
    #expect(call?.sampleRate == 22050)
    #expect(call?.text == "The reply here.")
}

@Test func bargeInCancelsSpeechAndStartsANewTurn() async throws {
    let spoken = SpokenLog()
    let backends = VoiceLoopBackends(
        transcribe: { _ in textStream(["again"]) },
        chat: { _ in textStream(["A very long spoken answer arrives."]) },
        speak: { sentence in
            AsyncThrowingStream { continuation in
                let task = Task {
                    let call = await spoken.note(sentence)
                    continuation.yield(
                        .audio(AudioFrame(data: Data([9]), sampleRate: 24000)))
                    if call == 1 {
                        do {
                            try await Task.sleep(for: .seconds(300))
                        } catch {
                            continuation.finish(throwing: CancellationError())
                            return
                        }
                    }
                    continuation.yield(.done(nil))
                    continuation.finish()
                }
                continuation.onTermination = { _ in task.cancel() }
            }
        })

    let loop = VoiceLoop(backends: backends, vadConfig: tinyVAD)
    let log = EventLog()
    let events = await loop.start()
    let collector = Task {
        for await event in events {
            await log.record(event)
        }
    }

    await loop.feed(voiced + voiced + voiced)
    await loop.feed(quiet + quiet + quiet)
    try await waitUntil { await spoken.sentences.count == 1 }

    await loop.feed(voiced + voiced + voiced)
    try await waitUntil { await log.bargeIns() >= 1 }
    await loop.feed(quiet + quiet + quiet)
    try await waitUntil { await log.completions() == 1 }
    await loop.stop()
    _ = await collector.value

    #expect(await log.userTurns() == ["again", "again"])
    #expect(await spoken.sentences.count == 2)
    #expect(await log.completions() == 1)
}

@Test func secondStartFinishesTheFirstEventStream() async throws {
    let backends = VoiceLoopBackends(
        transcribe: { _ in textStream(["hi"]) },
        chat: { _ in textStream(["hello"]) },
        speak: { _ in textStream([]) })
    let loop = VoiceLoop(backends: backends, vadConfig: tinyVAD)

    let first = await loop.start()
    let firstEnded = CleanupFlag()
    let collector = Task {
        for await _ in first {}
        firstEnded.mark()
    }

    let second = await loop.start()
    _ = await collector.value
    #expect(firstEnded.wasInvoked)

    var iterator = second.makeAsyncIterator()
    let event = await iterator.next()
    if case .listening = event {} else { Issue.record("expected .listening, got \(String(describing: event))") }
    await loop.stop()
}
