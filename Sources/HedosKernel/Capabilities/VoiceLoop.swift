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
    public var persistTurnAudio: (@Sendable (Data, Int, String) async -> Void)?

    public init(
        transcribe: @escaping @Sendable ([Float]) async throws -> AsyncThrowingStream<
            CapabilityChunk, Error
        >,
        chat: @escaping @Sendable (String) async throws -> AsyncThrowingStream<
            CapabilityChunk, Error
        >,
        speak: @escaping @Sendable (String) async throws -> AsyncThrowingStream<
            CapabilityChunk, Error
        >,
        persistTurnAudio: (@Sendable (Data, Int, String) async -> Void)? = nil
    ) {
        self.transcribe = transcribe
        self.chat = chat
        self.speak = speak
        self.persistTurnAudio = persistTurnAudio
    }
}

struct VoiceLoopBackendAdapter: PipelineBackend {
    let backends: VoiceLoopBackends

    func invoke(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> AsyncThrowingStream<CapabilityChunk, Error>
    {
        switch capability {
        case .transcribe:
            guard case .object(let object) = payload,
                case .string(let base64)? = object["pcm"],
                let data = Data(base64Encoded: base64)
            else { return VoiceLoopBackendAdapter.empty() }
            let samples = data.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
            return try await backends.transcribe(samples)
        case .speak:
            guard case .object(let object) = payload,
                case .string(let text)? = object["text"]
            else { return VoiceLoopBackendAdapter.empty() }
            return try await backends.speak(text)
        default:
            return try await backends.chat(VoiceLoopBackendAdapter.userText(payload))
        }
    }

    func submit(_ modelID: String, _ capability: Capability, payload: JSONValue) async throws
        -> String
    {
        throw KernelError.runtimeFailed("voice loop stages do not run jobs")
    }

    func jobEvents(id: String) async -> AsyncStream<JobEvent> {
        AsyncStream { $0.finish() }
    }

    func cancel(jobID: String) async {}

    func artifactData(id: String) async throws -> Data? { nil }

    static func userText(_ payload: JSONValue) -> String {
        guard case .object(let object) = payload,
            case .array(let messages)? = object["messages"]
        else { return "" }
        for entry in messages.reversed() {
            guard case .object(let fields) = entry,
                case .string(let content)? = fields["content"]
            else { continue }
            return content
        }
        return ""
    }

    static func empty() -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream { $0.finish() }
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
        stop()
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

    private func stages() -> [PipelineStageRunner] {
        let backend = VoiceLoopBackendAdapter(backends: backends)
        return [
            PipelineRunnerFactory.transcribe(
                index: 0, modelID: "voice-transcribe", params: [:], sampleRate: 16000,
                backend: backend),
            PipelineRunnerFactory.textToText(
                index: 1, modelID: "voice-chat", capability: .chat, params: [:],
                backend: backend),
            PipelineRunnerFactory.speak(
                index: 2, modelID: "voice-speak", params: [:], voice: nil, backend: backend),
        ]
    }

    private func runTurn(_ samples: [Float]) async {
        defer { turnTask = nil }
        continuation?.yield(.status("transcribing"))
        var reportedTurn = false
        var pcm = Data()
        var sampleRate = SidecarSpec.defaultSampleRate
        var assistantText = ""
        for await event in PipelineExecutor(stages: stages()).run(input: .audio(samples)) {
            if Task.isCancelled { return }
            switch event {
            case .transcript(_, let text):
                let question = text.trimmingCharacters(in: .whitespacesAndNewlines)
                if question.isEmpty {
                    continuation?.yield(.listening)
                    return
                }
                reportedTurn = true
                continuation?.yield(.userTurn(question))
            case .delta(_, let delta):
                assistantText += delta
                continuation?.yield(.assistantDelta(delta))
            case .audio(let frame):
                if pcm.isEmpty { sampleRate = frame.sampleRate }
                pcm.append(frame.data)
                continuation?.yield(.speech(frame))
            case .completed:
                if reportedTurn {
                    if !pcm.isEmpty {
                        await backends.persistTurnAudio?(pcm, sampleRate, assistantText)
                    }
                    continuation?.yield(.turnCompleted)
                }
                continuation?.yield(.listening)
            case .cancelled:
                continuation?.yield(.listening)
            case .failed(let message):
                continuation?.yield(.failed(message))
                continuation?.yield(.listening)
            case .stageStarted, .status, .artifact, .vector:
                break
            }
        }
    }
}

extension Kernel {
    public func voiceLoop(
        sessionID: String, transcriberID: String, speakerID: String, voice: String,
        speed: Double? = nil
    ) -> VoiceLoop {
        let effectiveSpeed = speed ?? 1.0
        return VoiceLoop(
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
                            "speed": .double(effectiveSpeed),
                        ]))
                },
                persistTurnAudio: { pcm, sampleRate, assistantText in
                    guard let transcript = try? await self.chats.session(id: sessionID),
                        let target = transcript.turns.last(where: {
                            $0.supersededBy == nil && $0.role == .assistant
                                && !$0.content.isEmpty
                        })
                    else { return }
                    guard
                        let artifact = try? await self.saveSpeech(
                            modelID: speakerID, voice: voice, text: assistantText,
                            speed: effectiveSpeed, sampleRate: sampleRate, pcm: pcm,
                            sessionID: sessionID)
                    else { return }
                    try? await self.replaceSpokenArtifact(
                        sessionID: sessionID, turnID: target.id, artifactID: artifact.id)
                }))
    }
}
