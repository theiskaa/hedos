import AppKit
import HedosKernel
import SwiftUI

private struct TranscriptTailKey: Equatable {
    var count: Int
    var lastID: String?

    init(_ transcript: [ChatViewModel.Entry]) {
        count = transcript.count
        lastID = transcript.last?.id
    }
}

@Observable
@MainActor
final class ChatViewModel {
    struct Version: Identifiable, Hashable {
        var id: String
        var text: String
        var thinking: String
        var stats: GenerationStats?
        var artifactRefs: [String] = []
    }

    struct Entry: Identifiable, Hashable {
        var id: String = UUID().uuidString
        var role: TurnRole
        var text: String
        var thinking: String = ""
        var stats: GenerationStats?
        var artifactRefs: [String] = []
        var persisted = false
        var generatesArtifact = false
        var interrupted = false
        var versions: [Version] = []
    }

    enum Intent: Hashable, CaseIterable {
        case text
        case image
        case speak

        var capability: Capability {
            switch self {
            case .text: .chat
            case .image: .image
            case .speak: .speak
            }
        }

        init(_ intent: ChatIntent) {
            switch intent {
            case .text: self = .text
            case .image: self = .image
            case .speak: self = .speak
            }
        }

        var stored: ChatIntent {
            switch self {
            case .text: .text
            case .image: .image
            case .speak: .speak
            }
        }
    }

    enum ImagePhase: Equatable {
        case idle
        case queued(String?)
        case preparing
        case running
        case failed(String)
    }

    private let kernel: Kernel
    private let audio: AudioSession
    let sessionID: String
    private var streamTask: Task<Void, Never>?
    private var streamGeneration = 0
    private var readAloudTask: Task<Void, Never>?
    private var previewTask: Task<Void, Never>?
    private var speakTask: Task<Void, Never>?
    private var speakLiveID: String?
    private var imageTask: Task<Void, Never>?
    private var cancelRequested = false
    private var submittedPayload: JSONValue?
    var previewingVoice: String?
    var pendingSpeech: String?
    var isSpeaking = false
    var imagePhase: ImagePhase = .idle
    var imageProgress: JobProgress = .none
    var imagePreview: NSImage?
    var activeImagePrompt: String?
    var jobID: String?

    var transcript: [Entry] = []
    var draft = ""
    var isStreaming = false
    var isTranscribing = false
    var isVisible = true
    var notice: String?
    var place: String?
    var canStartOllama = false
    var boundModelID: String?
    var defaultModelID: String?
    var intent: Intent = .text
    var imageModelID: String?
    var voiceModelID: String?
    var form = ParamForm(schema: [])
    var voice = "af_heart"
    var voices: [String] = []
    var speakingEntryID: String?
    var showsStreamCursor = false
    var streamStatus: String?
    var onSessionsChanged: (() -> Void)?
    var recordsProvider: (() -> [ModelRecord])?
    private var reveal = PacedReveal()
    private var lastDeltaAt = ContinuousClock().now
    private var tickerTask: Task<Void, Never>?
    private(set) var liveBalancedText = ""
    private static let liveBalanceThrottleTicks = 7
    private var liveBalanceThrottle = RefreshThrottle(everyTicks: liveBalanceThrottleTicks)

    init(kernel: Kernel, session: ChatSession, audio: AudioSession) {
        self.kernel = kernel
        self.audio = audio
        self.sessionID = session.id
        self.boundModelID = session.modelID
        self.intent = Intent(session.intent)
        self.imageModelID = session.imageModelID
        self.voiceModelID = session.voiceModelID
    }

    func load() async {
        if !isStreaming, let stored = try? await kernel.chats.session(id: sessionID) {
            apply(stored)
        }
        defaultModelID = await kernel.settings.defaultChatModelID()
        if await kernel.chats.persistenceDegraded() {
            notice = "This conversation isn't being saved right now."
        }
    }

    private func apply(_ stored: ChatTranscript) {
        boundModelID = stored.session.modelID
        intent = Intent(stored.session.intent)
        imageModelID = stored.session.imageModelID
        voiceModelID = stored.session.voiceModelID
        var retired: [String: [ChatTurn]] = [:]
        for turn in stored.turns {
            if let supersededBy = turn.supersededBy {
                retired[supersededBy, default: []].append(turn)
            }
        }
        func predecessor(of turn: ChatTurn) -> ChatTurn? {
            guard let candidate = retired[turn.id]?.min(by: { $0.seq < $1.seq }),
                candidate.role == turn.role
            else { return nil }
            return candidate
        }

        place = stored.session.place
        let active = stored.turns.filter {
            $0.supersededBy == nil && $0.role != .system
                && !($0.role == .assistant && $0.content.isEmpty && $0.toolCallsJSON != nil)
        }
        let placed = active.enumerated().map { index, turn -> (root: Int, entry: Entry) in
            var chain: [ChatTurn] = []
            var cursor = predecessor(of: turn)
            while let older = cursor, chain.count < 64 {
                chain.append(older)
                cursor = predecessor(of: older)
            }
            let history = chain.reversed() + [turn]
            let generates =
                turn.role == .user && index + 1 < active.count
                && active[index + 1].isGeneratedArtifact
            let entry = Entry(
                id: turn.id,
                role: turn.role,
                text: turn.content,
                thinking: turn.thinking ?? "",
                stats: turn.stats,
                artifactRefs: turn.artifactRefs,
                persisted: true,
                generatesArtifact: generates,
                interrupted: turn.interrupted,
                versions: history.map {
                    Version(
                        id: $0.id, text: $0.content, thinking: $0.thinking ?? "",
                        stats: $0.stats, artifactRefs: $0.artifactRefs)
                })
            return (history.first?.seq ?? turn.seq, entry)
        }
        transcript = placed.sorted { $0.root < $1.root }.map(\.entry)
        recountTranscriptCharacters()
    }

    private(set) var transcriptCharacterCount = 0

    private func recountTranscriptCharacters() {
        transcriptCharacterCount = transcript.reduce(0) { $0 + $1.text.count }
    }

    var isWorking: Bool {
        isStreaming || isSpeaking || imageBusy
    }

    var imageBusy: Bool {
        switch imagePhase {
        case .queued, .preparing, .running: true
        case .idle, .failed: false
        }
    }

    var activeModelID: String? {
        switch intent {
        case .text: boundModelID
        case .image: imageModelID
        case .speak: voiceModelID
        }
    }

    func imageModels(in records: [ModelRecord]) -> [ModelRecord] {
        Launcher.models(in: records, for: .images).filter {
            Launcher.destination(for: $0) == .images
        }
    }

    func waitingImageModels(in records: [ModelRecord]) -> [ModelRecord] {
        Launcher.models(in: records, for: .images).filter {
            Launcher.destination(for: $0) != .images
        }
    }

    func voiceModels(in records: [ModelRecord]) -> [ModelRecord] {
        Launcher.models(in: records, for: .voice).filter {
            Launcher.destination(for: $0) == .voice
        }
    }

    func setIntent(_ next: Intent) {
        guard intent != next, !isWorking else { return }
        if case .failed = imagePhase {
            imagePhase = .idle
        }
        intent = next
        ensureBinding(for: next)
        let kernel = kernel
        let sessionID = sessionID
        Task { try? await kernel.chats.setIntent(id: sessionID, intent: next.stored) }
    }

    private func ensureBinding(for intent: Intent) {
        let records = recordsProvider?() ?? []
        switch intent {
        case .text:
            break
        case .image:
            let runnable = imageModels(in: records)
            if !runnable.contains(where: { $0.id == imageModelID }), let first = runnable.first {
                bindImage(to: first)
            }
        case .speak:
            let speakers = voiceModels(in: records)
            if !speakers.contains(where: { $0.id == voiceModelID }),
                let first = SpeechModels.preferred(in: records) ?? speakers.first
            {
                Task { await bindVoice(to: first) }
            }
        }
    }

    func selectVoice(_ candidate: String) {
        guard voice != candidate else { return }
        voice = candidate
        let kernel = kernel
        Task {
            var settings = await kernel.settings.voice()
            settings.defaultVoice = candidate
            try? await kernel.settings.save(settings)
        }
    }

    func previewVoice(_ candidate: String) {
        if previewingVoice == candidate {
            stopVoicePreview()
            return
        }
        previewTask?.cancel()
        guard let voiceModelID else { return }
        previewingVoice = candidate
        let liveID = "preview-\(candidate)"
        audio.beginLive(
            AudioSession.Track(
                id: liveID, title: SpeechModels.previewLine, subtitle: candidate),
            audible: true,
            onStop: { [weak self] in self?.stopVoicePreview() })
        previewTask = Task { [weak self] in
            guard let self else { return }
            do {
                let stream = try await kernel.invoke(
                    voiceModelID, .speak,
                    payload: .object([
                        "text": .string(SpeechModels.previewLine),
                        "voice": .string(candidate),
                    ]))
                for try await chunk in stream {
                    if case .audio(let frame) = chunk {
                        audio.enqueue(frame, for: liveID)
                    }
                }
            } catch is CancellationError {
            } catch {
                notice = error.localizedDescription
            }
            if previewingVoice == candidate {
                previewingVoice = nil
            }
            audio.finishLive(liveID)
        }
    }

    func bindImage(to record: ModelRecord, persist: Bool = true) {
        guard imageModelID != record.id else { return }
        imageModelID = record.id
        form = ParamForm(schema: record.params)
        guard persist else { return }
        let kernel = kernel
        let sessionID = sessionID
        let recordID = record.id
        Task { try? await kernel.chats.bindImageModel(id: sessionID, modelID: recordID) }
    }

    func bindVoice(to record: ModelRecord, persist: Bool = true) async {
        guard voiceModelID != record.id || voices.isEmpty else { return }
        let changed = voiceModelID != record.id
        voiceModelID = record.id
        if persist && changed {
            try? await kernel.chats.bindVoiceModel(id: sessionID, modelID: record.id)
        }
        voices = (try? await kernel.voices(for: record.id)) ?? []
        let fallback = await kernel.settings.voice().defaultVoice
        if case .string(let configured)? = record.paramValues["voice"],
            voices.contains(configured)
        {
            voice = configured
        } else if let fallback, voices.contains(fallback) {
            voice = fallback
        } else if !voices.contains(voice), let first = voices.first {
            voice = first
        }
    }

    func adoptBindings(in records: [ModelRecord]) async {
        let images = imageModels(in: records)
        if let wanted = imageModelID, let record = images.first(where: { $0.id == wanted }) {
            if form.schema.isEmpty {
                form = ParamForm(schema: record.params)
            }
        } else if let record = images.first {
            bindImage(to: record, persist: false)
        }
        let speakers = voiceModels(in: records)
        if let wanted = voiceModelID, let record = speakers.first(where: { $0.id == wanted }) {
            if voices.isEmpty {
                await bindVoice(to: record, persist: false)
            }
        } else if let record = SpeechModels.preferred(in: records) {
            await bindVoice(to: record, persist: false)
        }
        if !records.isEmpty {
            let strandedImage = intent == .image && images.isEmpty
            let strandedVoice = intent == .speak && speakers.isEmpty
            if strandedImage || strandedVoice {
                intent = .text
                try? await kernel.chats.setIntent(id: sessionID, intent: Intent.text.stored)
            }
        }
    }

    func send() {
        switch intent {
        case .text: sendText()
        case .image: generateImage()
        case .speak: speakDraft()
        }
    }

    private func sendText() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isStreaming, boundModelID != nil else { return }
        draft = ""
        transcript.append(Entry(role: .user, text: text))
        transcriptCharacterCount += text.count
        stream { kernel, sessionID in
            try await kernel.sendChat(sessionID: sessionID, text: text)
        }
    }

    private func generateImage() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !isWorking, let modelID = imageModelID else { return }
        draft = ""
        let kernel = kernel
        let payload = form.payload(prompt: text)
        startImageJob(prompt: text, payload: payload) {
            try await kernel.submit(modelID, .image, payload: payload)
        }
    }

    func rerunImage(_ artifact: Artifact) {
        submitDerivedImage(from: artifact) { kernel, artifactID in
            try await kernel.rerun(artifactID: artifactID)
        }
    }

    func varyImage(_ artifact: Artifact) {
        submitDerivedImage(from: artifact) { kernel, artifactID in
            try await kernel.vary(artifactID: artifactID)
        }
    }

    private func submitDerivedImage(
        from artifact: Artifact,
        _ submit: @escaping @Sendable (Kernel, String) async throws -> String
    ) {
        guard !isWorking else { return }
        guard let record = recordsProvider?().first(where: { $0.id == artifact.modelID }),
            Launcher.destination(for: record) == .images
        else {
            notice = "The model that made this image is no longer runnable."
            return
        }
        bindImage(to: record)
        form.load(artifact.params)
        let kernel = kernel
        let artifactID = artifact.id
        let prompt = Provenance.prompt(of: artifact.params) ?? ""
        startImageJob(prompt: prompt, payload: artifact.params) {
            try await submit(kernel, artifactID)
        }
    }

    private func startImageJob(
        prompt: String, payload: JSONValue,
        _ submit: @escaping @Sendable () async throws -> String
    ) {
        notice = nil
        jobID = nil
        submittedPayload = payload
        activeImagePrompt = prompt
        cancelRequested = false
        imagePhase = .queued(nil)
        streamStatus = nil
        imageProgress = .none
        imagePreview = nil
        imageTask?.cancel()
        imageTask = Task { [weak self] in
            do {
                let id = try await submit()
                guard let self else { return }
                self.jobID = id
                if self.cancelRequested {
                    await self.kernel.scheduler.cancel(id)
                }
                await self.watchImage(id, prompt: prompt)
            } catch {
                guard let self else { return }
                self.imagePhase =
                    self.cancelRequested ? .idle : .failed(error.localizedDescription)
                self.activeImagePrompt = nil
            }
        }
    }

    private func watchImage(_ id: String, prompt: String) async {
        for await event in await kernel.scheduler.events(id: id) {
            switch event {
            case .queued(let reason):
                imagePhase = .queued(reason)
            case .preparing:
                imagePhase = .preparing
            case .status(let message):
                streamStatus = message
            case .running:
                imagePhase = .running
                streamStatus = nil
            case .progress(let updated):
                imageProgress = updated
            case .preview(let frame):
                imagePreview = NSImage(data: frame)
            case .done(let result):
                await landImage(result, prompt: prompt)
            case .failed(let message):
                imagePhase = .failed(message)
            case .cancelled:
                imagePhase = .idle
            }
        }
        if imageBusy {
            imagePhase = .idle
        }
        streamStatus = nil
        imagePreview = nil
        imageProgress = .none
        activeImagePrompt = nil
    }

    private func landImage(_ result: [String], prompt: String) async {
        imagePhase = .idle
        guard let artifactID = result.first else { return }
        try? await kernel.chats.appendGeneratedTurn(
            prompt: prompt, artifactID: artifactID,
            capabilityTag: SessionTag.generatedImage, to: sessionID)
        await reload()
        Haptics.completion()
    }

    func cancelImage() {
        guard imageBusy else { return }
        cancelRequested = true
        guard let jobID else { return }
        let kernel = kernel
        Task { await kernel.scheduler.cancel(jobID) }
    }

    func copyImageFailureDetails() {
        guard case .failed(let message) = imagePhase else { return }
        let details = Provenance.failureDetails(
            model: imageModelID ?? "",
            error: message,
            jobID: jobID,
            params: submittedPayload ?? form.payload(prompt: activeImagePrompt ?? ""),
            schema: form.schema)
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(details, forType: .string)
    }

    private func speakDraft() {
        let content = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !content.isEmpty, !isWorking, let modelID = voiceModelID else { return }
        draft = ""
        notice = nil
        pendingSpeech = content
        isSpeaking = true
        let liveID = "speak-\(UUID().uuidString)"
        speakLiveID = liveID
        speakTask = Task { [weak self] in
            guard let self else { return }
            var pcm = Data()
            var sampleRate = 24000
            let playsLive = await kernel.settings.voice().autoSpeak
            audio.beginLive(
                AudioSession.Track(id: liveID, title: content, subtitle: voice),
                audible: playsLive,
                onStop: { [weak self] in self?.stop() })
            do {
                var payload: [String: JSONValue] = [
                    "text": .string(content),
                    "voice": .string(voice),
                ]
                let record = recordsProvider?().first { $0.id == modelID }
                let effectiveSpeed: Double
                if case .double(let saved)? = record?.paramValues["speed"] {
                    effectiveSpeed = saved
                } else {
                    let speed = await kernel.settings.voice().speed
                    effectiveSpeed = speed
                    if speed != 1.0 {
                        payload["speed"] = .double(speed)
                    }
                }
                let stream = try await kernel.invoke(modelID, .speak, payload: .object(payload))
                for try await chunk in stream {
                    switch chunk {
                    case .status(let message):
                        streamStatus = message
                    case .audio(let frame):
                        streamStatus = nil
                        sampleRate = frame.sampleRate
                        pcm.append(frame.data)
                        audio.enqueue(frame, for: liveID)
                    default:
                        break
                    }
                }
                try Task.checkCancellation()
                if !pcm.isEmpty {
                    let artifact = try await kernel.saveSpeech(
                        modelID: modelID, voice: voice, text: content,
                        speed: effectiveSpeed, sampleRate: sampleRate, pcm: pcm,
                        sessionID: sessionID)
                    try await kernel.chats.appendGeneratedTurn(
                        prompt: content, artifactID: artifact.id,
                        capabilityTag: SessionTag.spoke, to: sessionID)
                    audio.finishLive(liveID)
                    await reload()
                    Haptics.completion()
                } else {
                    audio.finishLive(liveID)
                }
            } catch is CancellationError {
                audio.dismissIfActive(liveID)
            } catch {
                notice = error.localizedDescription
                draft = content
                audio.finishLive(liveID)
            }
            if speakLiveID == liveID {
                speakLiveID = nil
            }
            streamStatus = nil
            pendingSpeech = nil
            isSpeaking = false
        }
    }

    private func reload() async {
        _ = try? await kernel.autoTitleIfNeeded(sessionID: sessionID)
        if let stored = try? await kernel.chats.session(id: sessionID) {
            apply(stored)
        }
        onSessionsChanged?()
    }

    func rebind(to record: ModelRecord) {
        guard !isStreaming else { return }
        guard record.id != boundModelID else { return }
        let previous = boundModelID
        boundModelID = record.id
        let kernel = kernel
        let sessionID = sessionID
        let recordID = record.id
        let displayName = record.displayName
        Task {
            do {
                try await kernel.chats.rebindSession(id: sessionID, modelID: recordID)
            } catch {
                if boundModelID == recordID {
                    boundModelID = previous
                }
                notice = error.localizedDescription
                return
            }
            let assessment =
                (try? await kernel.chatContextAssessment(
                    sessionID: sessionID, modelID: recordID)) ?? nil
            if let assessment, !assessment.fits {
                notice =
                    "This conversation is longer than \(displayName) can hold — older turns will not fit. Start a new chat or pick a larger model."
            } else {
                notice = nil
            }
            onSessionsChanged?()
        }
    }

    func makeDefault(_ record: ModelRecord) {
        defaultModelID = record.id
        let kernel = kernel
        Task {
            try? await kernel.settings.setDefaultChatModelID(record.id)
        }
    }

    func edit(_ entry: Entry, text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !isStreaming, entry.role == .user, entry.persisted, !trimmed.isEmpty else { return }
        guard let index = transcript.firstIndex(where: { $0.id == entry.id }) else { return }
        transcript.removeSubrange(index...)
        transcript.append(Entry(role: .user, text: trimmed))
        recountTranscriptCharacters()
        stream { kernel, sessionID in
            try await kernel.editChatTurn(sessionID: sessionID, turnID: entry.id, text: trimmed)
        }
    }

    func regenerate(_ entry: Entry) {
        guard !isStreaming, entry.role == .assistant, entry.persisted else { return }
        guard let index = transcript.firstIndex(where: { $0.id == entry.id }) else { return }
        transcript.removeSubrange(index...)
        recountTranscriptCharacters()
        stream { kernel, sessionID in
            try await kernel.regenerateChatTurn(sessionID: sessionID, turnID: entry.id)
        }
    }

    func startOllamaAndRetry() {
        guard !isStreaming else { return }
        canStartOllama = false
        notice = "Starting Ollama…"
        Task {
            do {
                try await kernel.startOllama()
                notice = nil
                if transcript.last?.role == .user {
                    stream { kernel, sessionID in
                        try await kernel.continueChat(sessionID: sessionID)
                    }
                }
            } catch {
                notice = error.localizedDescription
            }
        }
    }

    func stop() {
        cancelImage()
        teardown()
    }

    func teardown() {
        streamTask?.cancel()
        isStreaming = false
        suspend()
    }

    func suspend() {
        isVisible = false
        stopTicker()
    }

    func resumeUI() {
        isVisible = true
        guard isStreaming else { return }
        startTicker()
    }

    func stopVoicePreview() {
        previewTask?.cancel()
        let candidate = previewingVoice
        previewingVoice = nil
        if let candidate, audio.isActive("preview-\(candidate)") {
            audio.dismiss()
        }
    }

    func transcribeDropped(_ url: URL, records: [ModelRecord]) {
        guard let transcriber = DictationController.transcriber(in: records) else {
            notice = "Transcribing a file needs a ready transcription model."
            return
        }
        guard url.pathExtension.lowercased() == "wav" else {
            notice = "Only WAV files can be transcribed for now."
            return
        }
        notice = "Transcribing \(url.lastPathComponent)…"
        isTranscribing = true
        Task { [weak self] in
            guard let self else { return }
            do {
                let stream = try await kernel.invoke(
                    transcriber.id, .transcribe,
                    payload: .object(["audio": .string(url.path)]))
                for try await chunk in stream {
                    switch chunk {
                    case .text(let delta), .segment(let delta, _, _):
                        draft += delta
                    default:
                        break
                    }
                }
                notice = nil
            } catch {
                notice = error.localizedDescription
            }
            isTranscribing = false
        }
    }

    private func tickReveal() {
        guard isStreaming else { return }
        if reveal.tick() {
            if liveBalancedText.isEmpty || liveBalanceThrottle.shouldRefresh() {
                refreshLiveBalance()
            }
        }
        let idle = ContinuousClock().now - lastDeltaAt
        let quiet = idle > .milliseconds(150)
        let cursor = isStreaming && quiet && reveal.backlog == 0 && reveal.revealedCount > 0
        if showsStreamCursor != cursor {
            showsStreamCursor = cursor
        }
        if streamStatus == nil, reveal.revealedCount == 0, idle > .seconds(8),
            transcript.last?.thinking.isEmpty != false
        {
            streamStatus = "Still waiting on the model — it may be loading"
        }
    }

    private func refreshLiveBalance() {
        liveBalancedText = MarkdownBalancer.balanced(reveal.revealed)
    }

    private func startTicker() {
        tickerTask?.cancel()
        tickerTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(16))
                guard let self else { return }
                self.tickReveal()
            }
        }
    }

    private func stopTicker() {
        tickerTask?.cancel()
        tickerTask = nil
        showsStreamCursor = false
    }

    private func stream(
        _ start: @escaping @Sendable (Kernel, String) async throws -> AsyncThrowingStream<
            CapabilityChunk, Error
        >
    ) {
        streamTask?.cancel()
        streamGeneration += 1
        let generation = streamGeneration
        notice = nil
        canStartOllama = false
        transcript.append(Entry(role: .assistant, text: ""))
        isStreaming = true
        reveal.reset()
        liveBalancedText = ""
        liveBalanceThrottle = RefreshThrottle(everyTicks: Self.liveBalanceThrottleTicks)
        lastDeltaAt = ContinuousClock().now
        startTicker()

        streamTask = Task { [weak self] in
            guard let self else { return }
            var pendingThinking = ""
            let clock = ContinuousClock()
            var lastThinkingFlush = clock.now

            @MainActor func flushThinking() {
                guard !transcript.isEmpty, !pendingThinking.isEmpty else { return }
                transcript[transcript.count - 1].thinking += pendingThinking
                pendingThinking = ""
            }

            @MainActor func settle() {
                guard generation == self.streamGeneration else { return }
                flushThinking()
                guard !transcript.isEmpty else { return }
                reveal.finish()
                transcript[transcript.count - 1].text = reveal.revealed
                refreshLiveBalance()
            }

            do {
                let stream = try await start(kernel, sessionID)
                for try await chunk in stream {
                    switch chunk {
                    case .text(let delta):
                        reveal.append(delta)
                        transcriptCharacterCount += delta.count
                        lastDeltaAt = clock.now
                        if streamStatus != nil {
                            streamStatus = nil
                        }
                    case .thinking(let delta):
                        pendingThinking += delta
                        if streamStatus != nil {
                            streamStatus = nil
                        }
                    case .status(let message):
                        streamStatus = message
                    case .toolCall(let call):
                        settle()
                        if let last = transcript.last, last.role == .assistant,
                            last.text.isEmpty, last.thinking.isEmpty
                        {
                            transcript.removeLast()
                        }
                        transcript.append(
                            Entry(role: .tool, text: Harness.actionSummary(call)))
                        transcript.append(Entry(role: .assistant, text: ""))
                        reveal.reset()
                        liveBalancedText = ""
                        streamStatus = "running \(call.name)"
                    case .done(let stats):
                        if !transcript.isEmpty {
                            transcript[transcript.count - 1].stats = stats
                        }
                    default:
                        break
                    }
                    if clock.now - lastThinkingFlush > .milliseconds(50) {
                        flushThinking()
                        lastThinkingFlush = clock.now
                    }
                }
                settle()
                if generation == self.streamGeneration { Haptics.completion() }
            } catch KernelError.runtimeUnavailable(let hint) {
                settle()
                if generation == self.streamGeneration {
                    notice = hint
                    canStartOllama = hint.contains("ollama serve")
                    dropEmptyAssistantTail()
                }
            } catch is CancellationError {
                settle()
                if generation == self.streamGeneration {
                    dropEmptyAssistantTail()
                }
            } catch {
                settle()
                if generation == self.streamGeneration {
                    notice = error.localizedDescription
                    dropEmptyAssistantTail()
                }
            }
            guard generation == self.streamGeneration else { return }
            stopTicker()
            streamStatus = nil
            isStreaming = false
            _ = try? await kernel.autoTitleIfNeeded(sessionID: sessionID)
            guard generation == self.streamGeneration else { return }
            let stored = try? await kernel.chats.session(id: sessionID)
            guard generation == self.streamGeneration else { return }
            if let stored {
                apply(stored)
            }
            onSessionsChanged?()
            autoSpeakIfWanted()
        }
    }

    func setPlace(_ path: String) {
        Task {
            do {
                try await kernel.setChatPlace(sessionID: sessionID, path: path)
                await load()
            } catch {
                notice = error.localizedDescription
            }
        }
    }

    func placeFiles() async -> [String] {
        guard let place else { return [] }
        return await Task.detached { PlaceFiles.list(place: place) }.value
    }

    func clearPlace() {
        Task {
            try? await kernel.setChatPlace(sessionID: sessionID, path: nil)
            place = nil
            await load()
        }
    }

    private func dropEmptyAssistantTail() {
        if let last = transcript.last, last.role == .assistant, last.text.isEmpty {
            transcript.removeLast()
        }
    }

    func speaker(in records: [ModelRecord]) -> ModelRecord? {
        if let voiceModelID,
            let bound = records.first(where: {
                $0.id == voiceModelID && $0.state == .ready
                    && Launcher.destination(for: $0) == .voice
            })
        {
            return bound
        }
        return SpeechModels.preferred(in: records)
    }

    func toggleReadAloud(_ entry: Entry) {
        if speakingEntryID == entry.id {
            stopReadAloud()
            return
        }
        stopReadAloud()
        guard entry.role == .assistant,
            let speaker = speaker(in: recordsProvider?() ?? [])
        else { return }
        let text = SpeechText.speakable(entry.text)
        guard !text.isEmpty else { return }
        speakingEntryID = entry.id
        let liveID = "narrate-\(entry.id)"
        readAloudTask = Task { [weak self] in
            guard let self else { return }
            var pcm = Data()
            var sampleRate = 24000
            do {
                let voices = (try? await kernel.voices(for: speaker.id)) ?? []
                var chosen: String?
                if speaker.id == voiceModelID, voices.contains(voice) {
                    chosen = voice
                } else if case .string(let configured)? = speaker.paramValues["voice"],
                    voices.contains(configured)
                {
                    chosen = configured
                } else if let fallback = await kernel.settings.voice().defaultVoice,
                    voices.contains(fallback)
                {
                    chosen = fallback
                } else {
                    chosen = voices.first
                }
                guard let voice = chosen else {
                    notice = "\(speaker.displayName) offers no voices to read with."
                    speakingEntryID = nil
                    return
                }
                var payload: [String: JSONValue] = [
                    "text": .string(text),
                    "voice": .string(voice),
                ]
                let effectiveSpeed: Double
                if case .double(let saved)? = speaker.paramValues["speed"] {
                    effectiveSpeed = saved
                } else {
                    let speed = await kernel.settings.voice().speed
                    effectiveSpeed = speed
                    if speed != 1.0 {
                        payload["speed"] = .double(speed)
                    }
                }
                audio.beginLive(
                    AudioSession.Track(id: liveID, title: text, subtitle: voice),
                    audible: true,
                    onStop: { [weak self] in self?.stopReadAloud() })
                let stream = try await kernel.invoke(
                    speaker.id, .speak, payload: .object(payload))
                for try await chunk in stream {
                    if case .audio(let frame) = chunk {
                        sampleRate = frame.sampleRate
                        pcm.append(frame.data)
                        audio.enqueue(frame, for: liveID)
                    }
                }
                if !pcm.isEmpty, entry.persisted, !Task.isCancelled {
                    if let artifact = try? await kernel.saveSpeech(
                        modelID: speaker.id, voice: voice, text: text,
                        speed: effectiveSpeed, sampleRate: sampleRate, pcm: pcm,
                        sessionID: sessionID)
                    {
                        try? await kernel.replaceSpokenArtifact(
                            sessionID: sessionID, turnID: entry.id, artifactID: artifact.id)
                        if let stored = try? await kernel.chats.session(id: sessionID) {
                            apply(stored)
                        }
                        onSessionsChanged?()
                    }
                }
                audio.finishLive(liveID)
            } catch is CancellationError {
                audio.dismissIfActive(liveID)
            } catch {
                notice = error.localizedDescription
                audio.finishLive(liveID)
            }
            if speakingEntryID == entry.id {
                speakingEntryID = nil
            }
        }
    }

    func stopReadAloud() {
        readAloudTask?.cancel()
        let entryID = speakingEntryID
        speakingEntryID = nil
        if let entryID, audio.isActive("narrate-\(entryID)") {
            audio.dismiss()
        }
    }

    func autoSpeakIfWanted() {
        let kernel = kernel
        Task { [weak self] in
            guard await kernel.settings.voice().autoSpeak else { return }
            guard let self, isVisible, !isStreaming, speakingEntryID == nil,
                let last = transcript.last, last.role == .assistant, !last.text.isEmpty
            else { return }
            toggleReadAloud(last)
        }
    }
}

struct ChatView: View {
    let session: ChatSession
    let library: LibraryViewModel
    let kernel: Kernel
    let audio: AudioSession
    let launch: ShellModel.PendingLaunch?
    let onOpenArtifacts: ((String) -> Void)?
    let onNewChat: (() -> Void)?
    let onLaunchConsumed: (() -> Void)?
    @Environment(\.chatShowsStats) private var showsStats
    @Environment(\.conversationWidth) private var conversationWidth
    @Environment(\.transcriptSpacing) private var transcriptSpacing
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Bindable var model: ChatViewModel
    @State private var followsStream = true
    @State private var scrolledUp = false
    @State private var expandedThinking: Set<String> = []
    @State private var versionSelection: [String: Int] = [:]
    @State private var editingEntryID: String?
    @State private var editText = ""
    @State private var modelMenuOpen = false
    @State private var voiceConversation = VoiceConversationController()
    @State private var copiedEntryID: String?
    @State private var showParams = false

    init(
        session: ChatSession, model: ChatViewModel, library: LibraryViewModel,
        kernel: Kernel,
        audio: AudioSession,
        launch: ShellModel.PendingLaunch? = nil,
        onOpenArtifacts: ((String) -> Void)? = nil,
        onNewChat: (() -> Void)? = nil,
        onLaunchConsumed: (() -> Void)? = nil
    ) {
        self.session = session
        self.model = model
        self.library = library
        self.kernel = kernel
        self.audio = audio
        self.launch = launch
        self.onOpenArtifacts = onOpenArtifacts
        self.onNewChat = onNewChat
        self.onLaunchConsumed = onLaunchConsumed
    }

    private var boundRecord: ModelRecord? {
        library.record(id: model.boundModelID)
    }

    private var boundReady: Bool {
        guard let record = boundRecord else { return false }
        return record.state == .ready && Launcher.destination(for: record) == .chat
    }

    private var activeRecord: ModelRecord? {
        library.record(id: model.activeModelID)
    }

    var body: some View {
        ConversationScaffold(
            placeholder: placeholder,
            draft: $model.draft,
            isWorking: model.isWorking,
            canSend: sendable,
            notice: contextNotice ?? model.notice,
            noticeActionLabel: model.canStartOllama ? "Start Ollama" : nil,
            noticeAction: model.canStartOllama ? { model.startOllamaAndRetry() } : nil,
            onSend: { model.send() },
            onStop: { model.stop() },
            slash: SlashSetup(
                kernel: kernel, capability: model.intent.capability, commands: slashCommands),
            mentions: MentionSetup(files: { await model.placeFiles() }),
            dictation: DictationSetup(
                kernel: kernel,
                records: { [weak library] in library?.records ?? [] }),
            transcript: { transcript },
            header: { composerHeader },
            aux: { composerAux },
            chip: { EmptyView() }
        )
        .task(id: session.id) {
            model.resumeUI()
            await model.load()
        }
        .task(id: library.shelfSignature) { await model.adoptBindings(in: library.records) }
        .task(id: launch) { await applyLaunch() }
        .dropDestination(for: URL.self) { urls, _ in
            guard let url = urls.first else { return false }
            model.transcribeDropped(url, records: library.records)
            return true
        }
        .onDisappear {
            model.suspend()
            model.stopReadAloud()
            model.stopVoicePreview()
            voiceConversation.stop()
        }
    }

    private func applyLaunch() async {
        guard let launch, let record = library.record(id: launch.modelID) else { return }
        if model.isWorking {
            model.stop()
        }
        switch launch.intent {
        case .text: model.rebind(to: record)
        case .image: model.bindImage(to: record)
        case .speak: await model.bindVoice(to: record)
        }
        model.setIntent(launch.intent)
        onLaunchConsumed?()
    }

    @ViewBuilder
    private var composerHeader: some View {
        HStack(spacing: Design.Space.s) {
            modelChip
            boundPlaceChip
        }
        .padding(.horizontal, Design.Space.xs)
    }

    @ViewBuilder
    private var boundPlaceChip: some View {
        if model.intent == .text, let place = model.place {
            HStack(spacing: Design.Space.xs) {
                Image(systemName: "folder")
                    .font(Design.micro)
                Text(URL(fileURLWithPath: place).lastPathComponent)
                    .font(Design.micro)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Button {
                    model.clearPlace()
                } label: {
                    Image(systemName: "xmark")
                        .font(Design.micro)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Stop reading this folder")
            }
            .foregroundStyle(.secondary)
            .padding(.horizontal, Design.Space.chipX)
            .frame(height: Design.Control.size)
            .background(Design.inkWash, in: Capsule())
            .overlay(
                Capsule()
                    .strokeBorder(Design.line, lineWidth: Design.hairlineWidth))
            .help(
                "The model can list, read, and search inside \(place). "
                    + "It cannot touch anything outside it, and it cannot write or run anything."
            )
        }
    }

    @ViewBuilder
    private var composerAux: some View {
        intentControl(
            .image, glyph: "photo", label: "Generate an image",
            available: !model.imageModels(in: library.records).isEmpty)
        intentControl(
            .speak, glyph: "speaker.wave.2", label: "Speak this text",
            available: !model.voiceModels(in: library.records).isEmpty)
        placeControl
        paramsControl
        voiceLoopControl
    }

    @ViewBuilder
    private var placeControl: some View {
        if model.intent == .text, model.place == nil {
            CircleControl(
                glyph: "folder",
                label: "Let the model read a folder"
            ) {
                pickPlace()
            }
        }
    }

    private func pickPlace() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Let the Model Read This Folder"
        panel.message =
            "The model will be able to list, read, and search files inside this folder "
            + "for this conversation. It cannot see anything outside it, and it cannot "
            + "write, delete, or run anything."
        if panel.runModal() == .OK, let url = panel.url {
            model.setPlace(url.path)
        }
    }

    @ViewBuilder
    private func intentControl(
        _ target: ChatViewModel.Intent, glyph: String, label: String, available: Bool
    ) -> some View {
        if available || model.intent == target {
            let active = model.intent == target
            CircleControl(
                glyph: glyph,
                prominent: active,
                label: active ? "Back to chat" : label
            ) {
                model.setIntent(active ? .text : target)
            }
            .disabled(model.isWorking)
            .accessibilityIdentifier("intent-\(target == .image ? "image" : "speak")")
        }
    }

    @ViewBuilder
    private var paramsControl: some View {
        if model.intent == .image && !model.form.schema.isEmpty {
            CircleControl(glyph: "slider.horizontal.3", label: "Generation parameters") {
                showParams.toggle()
            }
            .inkPopover(
                isPresented: $showParams,
                width: Design.Popover.form.width,
                maxHeight: Design.Popover.form.height
            ) {
                ParamsForm(form: Bindable(model).form, disabled: model.isWorking)
            }
        }
    }

    @ViewBuilder
    private var voiceLoopControl: some View {
        if VoiceConversationController.participants(in: library.records) != nil {
            if voiceConversation.active, let status = voiceConversation.status {
                ShimmerText(text: status, tracked: false)
                    .truncationMode(.tail)
                    .frame(maxWidth: Design.Column.control, alignment: .trailing)
            }
            CircleControl(
                glyph: voiceConversation.active ? "waveform.slash" : "waveform",
                prominent: voiceConversation.active,
                label: voiceConversation.active
                    ? "End voice conversation" : "Start voice conversation"
            ) {
                voiceConversation.toggle(
                    sessionID: session.id, kernel: kernel, records: library.records,
                    audio: audio
                ) { [weak model] in
                    Task { await model?.load() }
                }
            }
            .accessibilityIdentifier("voice-conversation")
        }
    }

    private var slashCommands: [SlashCommand] {
        var commands = [
            SlashCommand(
                id: "model", title: "model", subtitle: "Choose the chat model",
                glyph: "square.stack.3d.up"
            ) {
                modelMenuOpen = true
            }
        ]
        if let onNewChat {
            commands.append(
                SlashCommand(
                    id: "new", title: "new", subtitle: "Start a new chat", glyph: "plus.message",
                    perform: onNewChat))
        }
        return commands
    }

    private var contextNotice: String? {
        guard let record = library.records.first(where: { $0.id == model.boundModelID }),
            let window = ContextBudget.effectiveWindow(
                for: record,
                requestedContextLength: ContextBudget.storedContextLength(of: record))
        else { return nil }
        let estimated = ContextBudget.estimatedTokens(
            characters: model.transcriptCharacterCount)
        guard estimated * 5 > window * 4 else { return nil }
        return "This conversation is getting long; early turns may drop out of context."
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                VStack(alignment: .leading, spacing: transcriptSpacing) {
                    if model.transcript.isEmpty && model.pendingSpeech == nil && !model.imageBusy {
                        emptyTranscript
                    }
                    ForEach(Array(model.transcript.enumerated()), id: \.element.id) { index, entry in
                        turn(entry, at: index)
                    }
                    if let pending = model.pendingSpeech {
                        pendingSpeechRow(pending)
                    }
                    if model.imageBusy {
                        liveImageRow
                    }
                    if case .failed(let message) = model.imagePhase {
                        failedImageRow(message)
                    }
                    Color.clear.frame(height: 1).id("tail")
                }
                .padding(.horizontal, Design.Space.xxl)
                .padding(.vertical, Design.Space.xxl)
                .frame(maxWidth: conversationWidth, alignment: .leading)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onScrollGeometryChange(for: ScrollAnchorState.self) { geometry in
                ScrollAnchorState(
                    container: geometry.containerSize,
                    contentHeight: geometry.contentSize.height,
                    offsetY: geometry.contentOffset.y,
                    nearBottom: geometry.contentOffset.y + geometry.containerSize.height
                        >= geometry.contentSize.height - 60)
            } action: { old, new in
                if new.nearBottom {
                    followsStream = true
                } else if new.offsetY < old.offsetY - 8 {
                    followsStream = false
                }
                let scrollable = new.contentHeight > new.container.height + 8
                if scrolledUp != (scrollable && !new.nearBottom) {
                    scrolledUp = scrollable && !new.nearBottom
                }
                if followsStream
                    && (new.contentHeight != old.contentHeight || new.container != old.container)
                {
                    proxy.scrollTo("tail", anchor: .bottom)
                }
            }
            .onChange(of: TranscriptTailKey(model.transcript)) { old, new in
                if new.count > old.count {
                    followsStream = true
                    settleAtTail(proxy)
                }
            }
            .onChange(of: model.isStreaming) { _, streaming in
                if !streaming && followsStream {
                    settleAtTail(proxy)
                }
            }
            .onChange(of: model.pendingSpeech) { _, pending in
                if pending != nil {
                    followsStream = true
                    settleAtTail(proxy)
                }
            }
            .onChange(of: model.imageBusy) { _, busy in
                if busy {
                    followsStream = true
                    settleAtTail(proxy)
                }
            }
            .onAppear { settleAtTail(proxy) }
            .overlay(alignment: .bottomTrailing) {
                Group {
                    if scrolledUp {
                        Button {
                            followsStream = true
                            withAnimation(.easeOut(duration: 0.2)) {
                                proxy.scrollTo("tail", anchor: .bottom)
                            }
                        } label: {
                            Image(systemName: "chevron.down")
                                .font(Design.glyphInline.weight(.semibold))
                                .foregroundStyle(Design.inkSoft)
                                .frame(width: 30, height: 30)
                                .background(Design.surface, in: Circle())
                                .overlay(
                                    Circle().strokeBorder(
                                        Design.line, lineWidth: Design.hairlineWidth))
                                .shade(Design.Elevation.floating)
                        }
                        .buttonStyle(PressDipStyle())
                        .padding(.trailing, Design.Space.xxl)
                        .padding(.bottom, Design.Space.l)
                        .transition(.scale.combined(with: .opacity))
                        .accessibilityLabel("Scroll to bottom")
                    }
                }
                .animation(Design.wash, value: scrolledUp)
            }
        }
    }

    static func toolActionSummary(_ text: String) -> String {
        let firstLine = text.split(whereSeparator: \.isNewline).first.map(String.init) ?? text
        guard firstLine.hasPrefix("["),
            let dash = firstLine.range(of: " — data from the user's disk")
        else { return firstLine }
        return String(firstLine[firstLine.index(after: firstLine.startIndex)..<dash.lowerBound])
    }

    private struct ScrollAnchorState: Equatable {
        var container: CGSize = .zero
        var contentHeight: CGFloat = 0
        var offsetY: CGFloat = 0
        var nearBottom = false
    }

    private func settleAtTail(_ proxy: ScrollViewProxy) {
        proxy.scrollTo("tail", anchor: .bottom)
        Task {
            for delay in [80, 350] {
                try? await Task.sleep(for: .milliseconds(delay))
                proxy.scrollTo("tail", anchor: .bottom)
            }
        }
    }


    @ViewBuilder
    private func turn(_ entry: ChatViewModel.Entry, at index: Int) -> some View {
        if entry.role == .tool {
            ToolTimelineRow(
                summary: Self.toolActionSummary(entry.text),
                connectsUp: index > 0 && model.transcript[index - 1].role == .tool,
                connectsDown: index + 1 < model.transcript.count
                    && model.transcript[index + 1].role == .tool,
                gap: transcriptSpacing)
        } else if entry.role == .user {
            VStack(alignment: .trailing, spacing: 4) {
                if editingEntryID == entry.id {
                    editField(entry)
                } else {
                    PromptBubble(text: entry.text)
                        .contextMenu {
                            Button("Copy") { copy(entry.text) }
                            if entry.persisted && !model.isStreaming && !entry.generatesArtifact {
                                Button("Edit…") {
                                    editText = entry.text
                                    editingEntryID = entry.id
                                }
                            }
                        }
                }
                versionSwitcher(entry)
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.bottom, Design.Space.m)
        } else {
            VStack(alignment: .leading, spacing: 8) {
                if !displayThinking(entry).isEmpty {
                    thinkingBlock(entry)
                }
                if !displayText(entry).isEmpty {
                    MarkdownTurnView(text: displayText(entry), cursor: showsCursor(entry))
                        .contextMenu {
                            Button("Copy") { copy(displayText(entry)) }
                            if canReadAloud(entry) {
                                Button(
                                    model.speakingEntryID == entry.id
                                        ? "Stop Reading" : "Narrate"
                                ) {
                                    narrate(entry)
                                }
                            }
                            if entry.persisted && !model.isStreaming {
                                Button("Regenerate") { model.regenerate(entry) }
                            }
                        }
                } else if model.isStreaming && !entry.persisted && entry.thinking.isEmpty
                    && entry.id == model.transcript.last?.id
                {
                    HStack(spacing: Design.Space.chipX) {
                        TypingDots()
                        if let status = model.streamStatus {
                            ShimmerText(text: status, font: Design.caption, tracked: false)
                        }
                    }
                }
                if !displayText(entry).isEmpty && !(model.isStreaming && !entry.persisted) {
                    HStack(spacing: Design.Space.m) {
                        ArtifactTray {
                            TrayButton(
                                label: copiedEntryID == entry.id ? "Copied" : "Copy",
                                glyph: copiedEntryID == entry.id ? "checkmark" : "doc.on.doc"
                            ) {
                                copy(displayText(entry))
                                copiedEntryID = entry.id
                                Task {
                                    try? await Task.sleep(for: .seconds(1.5))
                                    if copiedEntryID == entry.id { copiedEntryID = nil }
                                }
                            }
                            if entry.persisted && !model.isStreaming {
                                TrayButton(label: "Regenerate", glyph: "arrow.clockwise") {
                                    model.regenerate(entry)
                                }
                            }
                            if canReadAloud(entry) {
                                TrayButton(
                                    label: model.speakingEntryID == entry.id ? "Stop" : "Speak",
                                    glyph: model.speakingEntryID == entry.id
                                        ? "stop.fill" : "speaker.wave.2"
                                ) {
                                    narrate(entry)
                                }
                            }
                        }
                        versionSwitcher(entry)
                        if model.speakingEntryID == entry.id {
                            SpeakingIndicator()
                        }
                        Spacer(minLength: 0)
                        if entry.interrupted {
                            Text("interrupted")
                                .font(Design.micro)
                                .tracking(Design.microTracking)
                                .foregroundStyle(Design.inkFaint)
                        }
                        if showsStats, let stats = displayStats(entry) {
                            statsLine(stats)
                        }
                    }
                }
                ForEach(displayArtifacts(entry), id: \.self) { reference in
                    artifactCard(reference)
                }
            }
            .frame(maxWidth: Design.Column.transcriptProse, alignment: .leading)
            .padding(.bottom, Design.Space.xl)
        }
    }

    private var liveImageRow: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            if let prompt = model.activeImagePrompt, !prompt.isEmpty {
                PromptBubble(text: prompt)
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            ImageDrawingCanvas(
                preview: model.imagePreview,
                progress: model.imageProgress,
                statusLine: imageStatusLine,
                onCancel: { model.cancelImage() })
        }
        .padding(.bottom, Design.Space.xl)
    }

    private var imageStatusLine: String? {
        switch model.imagePhase {
        case .queued(let reason): reason ?? model.streamStatus ?? "Waiting to run"
        case .preparing: "Preparing image runtime, first use only"
        default: model.streamStatus
        }
    }

    private func failedImageRow(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            Label {
                Text(message)
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
                    .lineSpacing(Design.bodyLineSpacing)
                    .frame(maxWidth: Design.Column.prose, alignment: .leading)
                    .textSelection(.enabled)
            } icon: {
                Image(systemName: "exclamationmark.triangle")
                    .font(Design.glyphInline.weight(.semibold))
                    .foregroundStyle(Design.inkSoft)
            }
            Button("Copy details") {
                model.copyImageFailureDetails()
            }
            .buttonStyle(QuietButtonStyle())
        }
        .responseShell()
        .padding(.bottom, Design.Space.xl)
    }

    private func pendingSpeechRow(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            PromptBubble(text: text)
                .frame(maxWidth: .infinity, alignment: .trailing)
            HStack(spacing: Design.Space.l) {
                SpeakingIndicator()
                if let status = model.streamStatus {
                    Text(status)
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                }
            }
            .responseShell()
        }
        .padding(.bottom, Design.Space.xl)
    }

    private func editField(_ entry: ChatViewModel.Entry) -> some View {
        VStack(alignment: .trailing, spacing: 6) {
            TextField("Edit message", text: $editText, axis: .vertical)
                .textFieldStyle(.plain)
                .font(Design.body)
                .lineLimit(1...8)
                .padding(.horizontal, Design.Space.l)
                .padding(.vertical, Design.Space.m)
                .background(Design.bubbleFill, in: RoundedRectangle.soft(Design.Radius.bubble))
                .overlay(
                    RoundedRectangle.soft(Design.Radius.bubble)
                        .strokeBorder(Design.hairline, lineWidth: Design.hairlineWidth))
            HStack(spacing: 8) {
                Button("Cancel") { editingEntryID = nil }
                    .buttonStyle(QuietButtonStyle())
                Button("Send") {
                    editingEntryID = nil
                    model.edit(entry, text: editText)
                }
                .buttonStyle(QuietButtonStyle())
                .disabled(
                    editText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
    }

    private func versionIndex(_ entry: ChatViewModel.Entry) -> Int {
        let last = max(entry.versions.count - 1, 0)
        return min(versionSelection[entry.id] ?? last, last)
    }

    private func selectedVersion(_ entry: ChatViewModel.Entry) -> ChatViewModel.Version? {
        guard !entry.versions.isEmpty else { return nil }
        return entry.versions[versionIndex(entry)]
    }

    @ViewBuilder
    private func versionSwitcher(_ entry: ChatViewModel.Entry) -> some View {
        if entry.versions.count > 1 {
            let index = versionIndex(entry)
            VersionSwitcher(
                index: index, count: entry.versions.count,
                onPrev: { versionSelection[entry.id] = index - 1 },
                onNext: { versionSelection[entry.id] = index + 1 })
        }
    }

    private func artifactCard(_ reference: String) -> some View {
        ArtifactExchangeView(
            reference: reference,
            kernel: kernel,
            session: audio,
            onRerun: model.isWorking ? nil : { model.rerunImage($0) },
            onVary: model.isWorking ? nil : { model.varyImage($0) }
        )
        .contextMenu {
            Button("Show in gallery") {
                onOpenArtifacts?(reference)
            }
        }
    }

    private func copy(_ text: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @ViewBuilder
    private func thinkingBlock(_ entry: ChatViewModel.Entry) -> some View {
        let streaming =
            entry.text.isEmpty && model.isStreaming && !entry.persisted
            && entry.id == model.transcript.last?.id
        return DisclosureGroup(
            isExpanded: Binding(
                get: { expandedThinking.contains(entry.id) },
                set: { expanded in
                    if expanded {
                        expandedThinking.insert(entry.id)
                    } else {
                        expandedThinking.remove(entry.id)
                    }
                })
        ) {
            Text(displayThinking(entry))
                .font(Design.label)
                .lineSpacing(Design.bodyLineSpacing)
                .foregroundStyle(Design.inkSoft)
                .textSelection(.enabled)
                .padding(.leading, Design.Space.l)
                .padding(.top, Design.Space.xs)
        } label: {
            if streaming {
                ShimmerText(text: "Thinking…", font: Design.label, tracked: false)
            } else {
                Text("Thought")
                    .font(Design.label)
                    .foregroundStyle(Design.inkFaint)
            }
        }
        .disclosureGroupStyle(QuietDisclosureStyle())
        .accessibilityLabel(streaming ? "Model thinking" : "Model thoughts")
    }

    private func statsLine(_ stats: GenerationStats) -> some View {
        GenerationStatsLine(stats: stats)
    }

    private func displayText(_ entry: ChatViewModel.Entry) -> String {
        let isLive =
            model.isStreaming && !entry.persisted && entry.id == model.transcript.last?.id
        guard isLive else { return selectedVersion(entry)?.text ?? entry.text }
        return model.liveBalancedText
    }

    private func displayThinking(_ entry: ChatViewModel.Entry) -> String {
        selectedVersion(entry)?.thinking ?? entry.thinking
    }

    private func displayStats(_ entry: ChatViewModel.Entry) -> GenerationStats? {
        selectedVersion(entry)?.stats ?? entry.stats
    }

    private func displayArtifacts(_ entry: ChatViewModel.Entry) -> [String] {
        selectedVersion(entry)?.artifactRefs ?? entry.artifactRefs
    }

    private func showsLatestVersion(_ entry: ChatViewModel.Entry) -> Bool {
        entry.versions.count < 2 || versionIndex(entry) == entry.versions.count - 1
    }

    private func showsCursor(_ entry: ChatViewModel.Entry) -> Bool {
        model.isStreaming && !entry.persisted && entry.id == model.transcript.last?.id
            && model.showsStreamCursor
    }

    private func canReadAloud(_ entry: ChatViewModel.Entry) -> Bool {
        entry.role == .assistant && !entry.text.isEmpty && !model.isStreaming
            && showsLatestVersion(entry)
            && displayArtifacts(entry).isEmpty
            && model.speaker(in: library.records) != nil
    }


    private func narrate(_ entry: ChatViewModel.Entry) {
        model.toggleReadAloud(entry)
    }

    private var emptyTranscript: some View {
        TranscriptEmptyState(
            eyebrow: "Chat · Local",
            headline: emptyHeadline,
            caption: emptyCaption)
    }

    private var emptyHeadline: String {
        switch model.intent {
        case .text:
            guard boundRecord != nil else { return "Pick a model to begin." }
            return boundReady ? "Say the first thing." : "That model isn't ready."
        case .image:
            return activeRecord != nil ? "Describe something." : "No image model yet."
        case .speak:
            return activeRecord != nil ? "Give it a line." : "No voice model yet."
        }
    }

    private var emptyCaption: String {
        switch model.intent {
        case .text:
            guard let record = boundRecord else {
                return "Every ready chat model on this Mac lives in the chip below the composer."
            }
            if !boundReady {
                return "\(record.displayName) isn't ready to run — pick another model from the chip below the composer."
            }
            return "\(record.displayName) is loaded and listening. Nothing you type leaves this Mac. Type / for saved prompts, or tap the mic to dictate."
        case .image:
            return activeRecord != nil
                ? "A sentence in, an image out — right here in the conversation. Steps, size, and seed live next to the send button."
                : "When an image model lands on your shelf, it draws into this conversation."
        case .speak:
            return activeRecord.map {
                "\($0.displayName) speaks your text into this conversation, playable and saveable. Preview voices from the chip below."
            } ?? "When a voice model lands on your shelf, it speaks from here."
        }
    }

    @ViewBuilder
    private var modelChip: some View {
        switch model.intent {
        case .text: chatChip
        case .image: imageChip
        case .speak:
            voiceChip
            voicePickerChip
        }
    }

    private var chatChipTitle: String {
        guard let record = boundRecord else { return "Choose model" }
        return boundReady ? record.displayName : "\(record.displayName) · not ready"
    }

    private var chatChip: some View {
        InkMenu(
            title: chatChipTitle,
            accessibilityName: "Chat model",
            readyDot: boundRecord != nil ? boundReady : nil,
            externalOpen: $modelMenuOpen,
            trigger: .chip
        ) {
            if chatGroups.isEmpty {
                InkMenuRow(title: "No chat-capable model is ready.", disabled: true) {}
            }
            ForEach(chatGroups, id: \.section) { group in
                InkMenuHeader(title: group.section)
                ForEach(group.records) { record in
                    InkMenuRow(
                        title: record.displayName,
                        annotation: menuAnnotation(record),
                        selected: record.id == model.boundModelID
                    ) {
                        model.rebind(to: record)
                    }
                }
            }
            if let bound = boundRecord, bound.id != model.defaultModelID {
                InkMenuDivider()
                InkMenuRow(title: "Make \(bound.displayName) the Default") {
                    model.makeDefault(bound)
                }
            }
        }
        .disabled(model.isWorking)
    }

    private var imageChip: some View {
        InkMenu(
            title: activeRecord?.displayName ?? "Choose model",
            accessibilityName: "Image model",
            readyDot: activeRecord != nil ? true : nil,
            trigger: .chip
        ) {
            let runnable = model.imageModels(in: library.records)
            let waiting = model.waitingImageModels(in: library.records)
            if runnable.isEmpty && waiting.isEmpty {
                InkMenuRow(title: "No image model is ready.", disabled: true) {}
            }
            ForEach(runnable) { record in
                InkMenuRow(
                    title: record.displayName,
                    selected: record.id == model.imageModelID
                ) {
                    model.bindImage(to: record)
                }
            }
            if !waiting.isEmpty {
                InkMenuDivider()
                ForEach(waiting) { record in
                    InkMenuRow(title: record.displayName, annotation: "needs recipe", disabled: true)
                    {}
                }
            }
        }
        .disabled(model.isWorking)
    }

    private var voiceChip: some View {
        InkMenu(
            title: activeRecord?.displayName ?? "Choose model",
            accessibilityName: "Voice model",
            readyDot: activeRecord != nil ? true : nil,
            trigger: .chip
        ) {
            let runnable = model.voiceModels(in: library.records)
            if runnable.isEmpty {
                InkMenuRow(title: "No voice model is ready.", disabled: true) {}
            }
            ForEach(runnable) { record in
                InkMenuRow(
                    title: record.displayName,
                    selected: record.id == model.voiceModelID
                ) {
                    Task { await model.bindVoice(to: record) }
                }
            }
        }
        .disabled(model.isWorking)
    }

    private var voicePickerChip: some View {
        InkMenu(title: model.voice, accessibilityName: "Voice", trigger: .chip) {
            ForEach(model.voices, id: \.self) { candidate in
                InkMenuRow(
                    title: candidate,
                    selected: candidate == model.voice,
                    previewing: model.previewingVoice == candidate,
                    onPreview: { model.previewVoice(candidate) }
                ) {
                    model.selectVoice(candidate)
                }
            }
        }
        .disabled(model.voices.isEmpty || model.isWorking)
    }

    private var chatGroups: [(section: String, records: [ModelRecord])] {
        LibraryViewModel.grouped(
            library.records.filter {
                $0.state == .ready && Launcher.destination(for: $0) == .chat
            })
    }

    private func menuAnnotation(_ record: ModelRecord) -> String? {
        var parts: [String] = []
        parts.append(MetaGrid.tierWord(record.runtime.tier))
        if let fit = Fit.short(record) {
            parts.append(fit)
        }
        if record.id == model.defaultModelID {
            parts.append("default")
        }
        return parts.joined(separator: " · ")
    }

    private var placeholder: String {
        switch model.intent {
        case .text:
            guard let record = boundRecord else { return "Pick a model first…" }
            return boundReady
                ? "Message \(record.displayName)…"
                : "\(record.displayName) isn't ready — pick another model…"
        case .image:
            return activeRecord != nil
                ? "What should this look like?" : "Pick an image model first…"
        case .speak:
            return activeRecord.map { "What should \($0.displayName) say?" }
                ?? "Pick a voice model first…"
        }
    }

    private var sendable: Bool {
        guard !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return false
        }
        switch model.intent {
        case .text:
            return boundReady
        case .image, .speak:
            return model.activeModelID != nil
        }
    }
}


private struct VersionSwitcher: View {
    let index: Int
    let count: Int
    let onPrev: () -> Void
    let onNext: () -> Void

    var body: some View {
        HStack(spacing: Design.Space.xs) {
            step(glyph: "chevron.left", label: "Previous version", enabled: index > 0, action: onPrev)
            Text("\(index + 1)/\(count)")
                .font(Design.micro)
                .monospacedDigit()
                .foregroundStyle(Design.inkFaint)
                .lineLimit(1)
                .fixedSize()
            step(
                glyph: "chevron.right", label: "Next version",
                enabled: index < count - 1, action: onNext)
        }
        .accessibilityLabel("Version \(index + 1) of \(count)")
    }

    private func step(
        glyph: String, label: String, enabled: Bool, action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            Image(systemName: glyph)
                .font(Design.glyphMicro)
                .foregroundStyle(enabled ? Design.inkSoft : Design.inkFaint.opacity(0.4))
                .frame(width: 14, height: 14)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .help(label)
        .accessibilityLabel(label)
    }
}

private struct GenerationStatsLine: View {
    let stats: GenerationStats

    var body: some View {
        Text(parts.joined(separator: " · "))
            .font(Design.micro)
            .tracking(Design.microTracking)
            .monospacedDigit()
            .foregroundStyle(Design.inkFaint)
            .lineLimit(1)
            .truncationMode(.tail)
            .layoutPriority(-1)
    }

    private var parts: [String] {
        var parts: [String] = []
        if let load = stats.loadMs, load > 0 {
            parts.append(String(format: "load %.1fs", Double(load) / 1000))
        }
        if let ttft = stats.ttftMs {
            parts.append(String(format: "ttft %.1fs", Double(ttft) / 1000))
        }
        if let tokens = stats.completionTokens {
            let tilde = stats.tokenCountsEstimated ? "~" : ""
            let generationMs = (stats.durationMs ?? 0) - (stats.ttftMs ?? 0)
            if generationMs > 0 {
                parts.append(
                    String(
                        format: "\(tilde)%.0f tok/s", Double(tokens) / Double(generationMs) * 1000))
            } else if let ms = stats.durationMs, ms > 0 {
                parts.append(
                    String(format: "\(tilde)%.0f tok/s", Double(tokens) / Double(ms) * 1000))
            }
            parts.append("\(tilde)\(tokens) tok")
        }
        return parts
    }
}
