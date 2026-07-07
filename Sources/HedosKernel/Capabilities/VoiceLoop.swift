import Foundation

public struct VADLite: Sendable {
    public struct Config: Sendable {
        public var sampleRate: Int
        public var hopSamples: Int
        public var speechRMS: Float
        public var startHops: Int
        public var endSilenceHops: Int
        public var minTurnHops: Int
        public var preRollHops: Int

        public init(
            sampleRate: Int = 16000,
            hopSamples: Int = 480,
            speechRMS: Float = 0.015,
            startHops: Int = 2,
            endSilenceHops: Int = 25,
            minTurnHops: Int = 8,
            preRollHops: Int = 10
        ) {
            self.sampleRate = sampleRate
            self.hopSamples = hopSamples
            self.speechRMS = speechRMS
            self.startHops = startHops
            self.endSilenceHops = endSilenceHops
            self.minTurnHops = minTurnHops
            self.preRollHops = preRollHops
        }
    }

    public enum Event: Sendable, Equatable {
        case speechStarted
        case turnEnded([Float])
    }

    private let config: Config
    private var pending: [Float] = []
    private var preRoll: [[Float]] = []
    private var turn: [Float] = []
    private var speaking = false
    private var voicedRun = 0
    private var silentRun = 0
    private var voicedHopsInTurn = 0

    public init(config: Config = Config()) {
        self.config = config
    }

    public mutating func consume(_ samples: [Float]) -> [Event] {
        pending.append(contentsOf: samples)
        var events: [Event] = []
        while pending.count >= config.hopSamples {
            let hop = Array(pending.prefix(config.hopSamples))
            pending.removeFirst(config.hopSamples)
            events.append(contentsOf: consumeHop(hop))
        }
        return events
    }

    private mutating func consumeHop(_ hop: [Float]) -> [Event] {
        let energy = Self.rms(hop)
        let voiced = energy >= config.speechRMS
        var events: [Event] = []

        if speaking {
            turn.append(contentsOf: hop)
            if voiced {
                silentRun = 0
                voicedHopsInTurn += 1
            } else {
                silentRun += 1
                if silentRun >= config.endSilenceHops {
                    if voicedHopsInTurn >= config.minTurnHops {
                        events.append(.turnEnded(turn))
                    }
                    speaking = false
                    turn = []
                    silentRun = 0
                    voicedRun = 0
                    voicedHopsInTurn = 0
                }
            }
            return events
        }

        preRoll.append(hop)
        if preRoll.count > config.preRollHops {
            preRoll.removeFirst()
        }
        if voiced {
            voicedRun += 1
            if voicedRun >= config.startHops {
                speaking = true
                turn = preRoll.flatMap { $0 }
                preRoll = []
                silentRun = 0
                voicedHopsInTurn = voicedRun
                events.append(.speechStarted)
            }
        } else {
            voicedRun = 0
        }
        return events
    }

    static func rms(_ samples: [Float]) -> Float {
        guard !samples.isEmpty else { return 0 }
        let sum = samples.reduce(Float(0)) { $0 + $1 * $1 }
        return (sum / Float(samples.count)).squareRoot()
    }
}

public struct SentenceChunker: Sendable {
    private var buffer = ""
    private let minLength: Int

    public init(minLength: Int = 12) {
        self.minLength = minLength
    }

    public mutating func consume(_ delta: String) -> [String] {
        buffer += delta
        var sentences: [String] = []
        while let boundary = nextBoundary() {
            let candidate = String(buffer[..<boundary])
                .trimmingCharacters(in: .whitespacesAndNewlines)
            buffer = String(buffer[boundary...])
            if !candidate.isEmpty {
                sentences.append(candidate)
            }
        }
        return sentences
    }

    public mutating func flush() -> String? {
        let remainder = buffer.trimmingCharacters(in: .whitespacesAndNewlines)
        buffer = ""
        return remainder.isEmpty ? nil : remainder
    }

    private func nextBoundary() -> String.Index? {
        var index = buffer.startIndex
        while index < buffer.endIndex {
            let character = buffer[index]
            if character == "\n" {
                let position = buffer.distance(from: buffer.startIndex, to: index)
                if position >= 1 {
                    return buffer.index(after: index)
                }
            }
            if ".!?".contains(character) {
                let after = buffer.index(after: index)
                let position = buffer.distance(from: buffer.startIndex, to: index)
                if position + 1 >= minLength,
                    after == buffer.endIndex || buffer[after].isWhitespace
                {
                    return after
                }
            }
            index = buffer.index(after: index)
        }
        return nil
    }
}

public struct VoiceLoopBackends: Sendable {
    public var transcribe:
        @Sendable ([Float]) async throws -> AsyncThrowingStream<CapabilityChunk, Error>
    public var chat:
        @Sendable (String) async throws -> AsyncThrowingStream<CapabilityChunk, Error>
    public var speak:
        @Sendable (String) async throws -> AsyncThrowingStream<CapabilityChunk, Error>

    public init(
        transcribe: @escaping @Sendable ([Float]) async throws -> AsyncThrowingStream<
            CapabilityChunk, Error
        >,
        chat: @escaping @Sendable (String) async throws -> AsyncThrowingStream<
            CapabilityChunk, Error
        >,
        speak: @escaping @Sendable (String) async throws -> AsyncThrowingStream<
            CapabilityChunk, Error
        >
    ) {
        self.transcribe = transcribe
        self.chat = chat
        self.speak = speak
    }
}

public actor VoiceLoop {
    public enum Event: Sendable {
        case listening
        case userSpeechBegan
        case userTurn(String)
        case assistantDelta(String)
        case speech(AudioFrame)
        case status(String)
        case turnCompleted
        case failed(String)
    }

    private let backends: VoiceLoopBackends
    private var vad: VADLite
    private var continuation: AsyncStream<Event>.Continuation?
    private var turnTask: Task<Void, Never>?
    private var running = false

    public init(backends: VoiceLoopBackends, vadConfig: VADLite.Config = VADLite.Config()) {
        self.backends = backends
        self.vad = VADLite(config: vadConfig)
    }

    public func start() -> AsyncStream<Event> {
        let (stream, continuation) = AsyncStream.makeStream(of: Event.self)
        self.continuation = continuation
        running = true
        continuation.yield(.listening)
        return stream
    }

    public func stop() {
        running = false
        turnTask?.cancel()
        turnTask = nil
        continuation?.finish()
        continuation = nil
    }

    public func feed(_ samples: [Float]) {
        guard running else { return }
        for event in vad.consume(samples) {
            switch event {
            case .speechStarted:
                if turnTask != nil {
                    turnTask?.cancel()
                    turnTask = nil
                }
                continuation?.yield(.userSpeechBegan)
            case .turnEnded(let turnSamples):
                beginTurn(turnSamples)
            }
        }
    }

    private func beginTurn(_ samples: [Float]) {
        turnTask?.cancel()
        turnTask = Task { [weak self] in
            await self?.runTurn(samples)
        }
    }

    private func runTurn(_ samples: [Float]) async {
        defer { turnTask = nil }
        do {
            continuation?.yield(.status("transcribing"))
            var heard = ""
            for try await chunk in try await backends.transcribe(samples) {
                if case .text(let delta) = chunk {
                    heard += delta
                }
            }
            let question = heard.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !question.isEmpty else {
                continuation?.yield(.listening)
                return
            }
            try Task.checkCancellation()
            continuation?.yield(.userTurn(question))

            let (sentences, sentenceFeed) = AsyncStream.makeStream(of: String.self)
            try await withThrowingTaskGroup(of: Void.self) { group in
                group.addTask { [backends, continuation] in
                    var chunker = SentenceChunker()
                    do {
                        for try await chunk in try await backends.chat(question) {
                            if case .text(let delta) = chunk {
                                continuation?.yield(.assistantDelta(delta))
                                for sentence in chunker.consume(delta) {
                                    sentenceFeed.yield(sentence)
                                }
                            }
                        }
                        if let rest = chunker.flush() {
                            sentenceFeed.yield(rest)
                        }
                        sentenceFeed.finish()
                    } catch {
                        sentenceFeed.finish()
                        throw error
                    }
                }
                group.addTask { [backends, continuation] in
                    for await sentence in sentences {
                        try Task.checkCancellation()
                        for try await chunk in try await backends.speak(sentence) {
                            if case .audio(let frame) = chunk {
                                continuation?.yield(.speech(frame))
                            }
                        }
                    }
                }
                try await group.waitForAll()
            }
            try Task.checkCancellation()
            continuation?.yield(.turnCompleted)
            continuation?.yield(.listening)
        } catch is CancellationError {
        } catch {
            continuation?.yield(.failed(error.localizedDescription))
            continuation?.yield(.listening)
        }
    }
}

extension Kernel {
    public func voiceLoop(
        sessionID: String, transcriberID: String, speakerID: String, voice: String
    ) -> VoiceLoop {
        VoiceLoop(
            backends: VoiceLoopBackends(
                transcribe: { samples in
                    let base64 = samples.withUnsafeBytes { Data($0) }.base64EncodedString()
                    return try await self.invoke(
                        transcriberID, .transcribe,
                        payload: .object([
                            "pcm": .string(base64),
                            "sampleRate": .int(WhisperEngine.expectedSampleRate),
                        ]))
                },
                chat: { text in
                    try await self.sendChat(sessionID: sessionID, text: text)
                },
                speak: { text in
                    try await self.invoke(
                        speakerID, .speak,
                        payload: .object([
                            "text": .string(text),
                            "voice": .string(voice),
                        ]))
                }))
    }
}
