import AVFoundation
import HedosKernel
import SwiftUI

@MainActor
final class PCMPlayer {
    private var engine: AVAudioEngine?
    private var player: AVAudioPlayerNode?
    private var format: AVAudioFormat?

    func enqueue(_ frame: AudioFrame) {
        let engine = self.engine ?? AVAudioEngine()
        let player = self.player ?? AVAudioPlayerNode()
        self.engine = engine
        self.player = player

        if format == nil || format?.sampleRate != Double(frame.sampleRate) {
            guard
                let newFormat = AVAudioFormat(
                    commonFormat: .pcmFormatFloat32,
                    sampleRate: Double(frame.sampleRate),
                    channels: 1,
                    interleaved: false)
            else { return }
            if engine.attachedNodes.contains(player) {
                engine.detach(player)
            }
            engine.attach(player)
            engine.connect(player, to: engine.mainMixerNode, format: newFormat)
            format = newFormat
        }
        guard let format else { return }

        let sampleCount = frame.data.count / MemoryLayout<Float>.size
        guard sampleCount > 0,
            let buffer = AVAudioPCMBuffer(
                pcmFormat: format, frameCapacity: AVAudioFrameCount(sampleCount))
        else { return }
        buffer.frameLength = AVAudioFrameCount(sampleCount)
        frame.data.withUnsafeBytes { raw in
            buffer.floatChannelData![0].update(
                from: raw.bindMemory(to: Float.self).baseAddress!, count: sampleCount)
        }

        if !engine.isRunning {
            try? engine.start()
        }
        player.scheduleBuffer(buffer)
        if !player.isPlaying {
            player.play()
        }
    }

    func stop() {
        player?.stop()
        engine?.stop()
        player = nil
        engine = nil
        format = nil
    }
}

private final class UtterancePlaybackDelegate: NSObject, AVAudioPlayerDelegate {
    var onFinish: (@Sendable () -> Void)?

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        onFinish?()
    }
}

@Observable
@MainActor
final class VoiceSurfaceModel {
    private let kernel: Kernel
    private var speakTask: Task<Void, Never>?
    private let player = PCMPlayer()
    private var audioPlayer: AVAudioPlayer?
    private let playbackDelegate = UtterancePlaybackDelegate()

    var utterances: [Artifact] = []
    var draft = ""
    var voice = "af_heart"
    var voices: [String] = []
    var boundModelID: String?
    private var boundSpeedIsConfigured = false
    var pendingText: String?
    var status: String?
    var notice: String?
    var isSpeaking = false
    var playingID: String?

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    func runnableModels(in records: [ModelRecord]) -> [ModelRecord] {
        Launcher.models(in: records, for: .voice).filter {
            Launcher.destination(for: $0) == .voice
        }
    }

    func start(records: [ModelRecord], preferring preferred: String?) async {
        let runnable = runnableModels(in: records)
        if boundModelID == nil || !runnable.contains(where: { $0.id == boundModelID }) {
            let candidate =
                runnable.first(where: { $0.id == preferred }) ?? runnable.first
            if let candidate {
                await bind(to: candidate)
            }
        }
        await load()
    }

    func bind(to record: ModelRecord) async {
        guard boundModelID != record.id || voices.isEmpty else { return }
        boundModelID = record.id
        boundSpeedIsConfigured = record.paramValues["speed"] != nil
        voices = (try? await kernel.voices(record.id)) ?? []
        let fallback = (try? await kernel.voiceSettings().defaultVoice) ?? nil
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

    func load() async {
        let all = (try? await kernel.artifacts()) ?? []
        utterances =
            all
            .filter { $0.capability == .speak }
            .sorted { $0.createdAt < $1.createdAt }
    }

    static func text(of artifact: Artifact) -> String {
        if case .object(let fields) = artifact.params,
            case .string(let value)? = fields["text"]
        {
            return value
        }
        return ""
    }

    static func voiceName(of artifact: Artifact) -> String? {
        if case .object(let fields) = artifact.params,
            case .string(let value)? = fields["voice"]
        {
            return value
        }
        return nil
    }

    static func peaks(of artifact: Artifact) -> [Double] {
        guard case .object(let fields) = artifact.params,
            case .array(let values)? = fields["peaks"]
        else { return [] }
        return values.compactMap {
            if case .double(let peak) = $0 { return peak }
            if case .int(let peak) = $0 { return Double(peak) }
            return nil
        }
    }

    func speak() {
        let content = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !content.isEmpty, !isSpeaking, let modelID = boundModelID else { return }
        draft = ""
        notice = nil
        pendingText = content
        isSpeaking = true
        stopPlayback()
        speakTask = Task { [weak self] in
            guard let self else { return }
            var pcm = Data()
            var sampleRate = 24000
            do {
                var payload: [String: JSONValue] = [
                    "text": .string(content),
                    "voice": .string(voice),
                ]
                if !boundSpeedIsConfigured {
                    let speed = (try? await kernel.voiceSettings().speed) ?? 1.0
                    if speed != 1.0 {
                        payload["speed"] = .double(speed)
                    }
                }
                let stream = try await kernel.invoke(
                    modelID, .speak, payload: .object(payload))
                for try await chunk in stream {
                    switch chunk {
                    case .status(let message):
                        status = message
                    case .audio(let frame):
                        status = nil
                        sampleRate = frame.sampleRate
                        pcm.append(frame.data)
                        player.enqueue(frame)
                    default:
                        break
                    }
                }
                if !pcm.isEmpty {
                    _ = try? await kernel.saveSpeech(
                        modelID: modelID, voice: voice, text: content,
                        sampleRate: sampleRate, pcm: pcm)
                    await load()
                }
            } catch is CancellationError {
            } catch {
                notice = error.localizedDescription
                draft = content
            }
            status = nil
            pendingText = nil
            isSpeaking = false
        }
    }

    func stop() {
        speakTask?.cancel()
        player.stop()
        status = nil
        pendingText = nil
        isSpeaking = false
    }

    func togglePlayback(_ artifact: Artifact) {
        if playingID == artifact.id {
            stopPlayback()
            return
        }
        stopPlayback()
        let kernel = kernel
        Task { @MainActor in
            guard let url = try? await kernel.artifactURL(id: artifact.id),
                let player = try? AVAudioPlayer(contentsOf: url)
            else { return }
            audioPlayer = player
            playingID = artifact.id
            playbackDelegate.onFinish = { [weak self] in
                Task { @MainActor in
                    self?.playingID = nil
                }
            }
            player.delegate = playbackDelegate
            player.play()
        }
    }

    private func stopPlayback() {
        audioPlayer?.stop()
        audioPlayer = nil
        playingID = nil
    }

    func download(_ artifact: Artifact) {
        let kernel = kernel
        Task { @MainActor in
            guard let url = try? await kernel.artifactURL(id: artifact.id) else { return }
            let panel = NSSavePanel()
            panel.nameFieldStringValue = url.lastPathComponent
            panel.begin { response in
                guard response == .OK, let destination = panel.url else { return }
                try? FileManager.default.removeItem(at: destination)
                try? FileManager.default.copyItem(at: url, to: destination)
            }
        }
    }

    func delete(_ artifact: Artifact) async {
        if playingID == artifact.id {
            stopPlayback()
        }
        try? await kernel.deleteArtifact(id: artifact.id)
        await load()
    }
}

struct VoiceSurface: View {
    @Bindable var shell: ShellModel
    @State private var deleting: Artifact?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.conversationWidth) private var conversationWidth
    @Environment(\.transcriptSpacing) private var transcriptSpacing

    private var model: VoiceSurfaceModel { shell.voice }
    private var boundRecord: ModelRecord? {
        shell.library.record(id: model.boundModelID)
    }

    var body: some View {
        @Bindable var model = shell.voice
        return ConversationScaffold(
            placeholder: placeholder,
            draft: $model.draft,
            isWorking: model.isSpeaking,
            canSend: speakable,
            notice: model.notice,
            onSend: { model.speak() },
            onStop: { model.stop() },
            transcript: { transcript },
            aux: {},
            chip: {
                modelChip
                voiceChip
            }
        )
        .task(id: shell.library.records.count) {
            await model.start(
                records: shell.library.records, preferring: shell.voiceSelection)
        }
        .confirmationDialog(
            "Move this recording to the Trash?",
            isPresented: Binding(
                get: { deleting != nil },
                set: { if !$0 { deleting = nil } })
        ) {
            Button("Move to Trash", role: .destructive) {
                if let artifact = deleting {
                    Task { await model.delete(artifact) }
                }
                deleting = nil
            }
        } message: {
            Text("The file moves to the Trash — it is not deleted outright.")
        }
    }

    private var transcript: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: transcriptSpacing) {
                    if model.utterances.isEmpty && model.pendingText == nil {
                        emptyTranscript
                    }
                    ForEach(model.utterances) { artifact in
                        utteranceRow(artifact)
                    }
                    if let pending = model.pendingText {
                        pendingRow(pending)
                    }
                    Color.clear.frame(height: 1).id("voice-tail")
                }
                .padding(.horizontal, Design.Space.xxl)
                .padding(.vertical, Design.Space.xxl)
                .frame(maxWidth: conversationWidth, alignment: .leading)
                .frame(maxWidth: .infinity)
            }
            .onChange(of: model.utterances.count) {
                proxy.scrollTo("voice-tail", anchor: .bottom)
            }
            .onChange(of: model.pendingText) {
                if model.pendingText != nil {
                    proxy.scrollTo("voice-tail", anchor: .bottom)
                }
            }
        }
    }

    private func utteranceRow(_ artifact: Artifact) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            PromptBubble(text: VoiceSurfaceModel.text(of: artifact))
            VoiceBubble(
                artifact: artifact,
                isPlaying: model.playingID == artifact.id
            ) {
                model.togglePlayback(artifact)
            }
            .contextMenu {
                Button("Download…") {
                    model.download(artifact)
                }
                Divider()
                Button("Delete…", role: .destructive) {
                    deleting = artifact
                }
            }
        }
    }

    private func pendingRow(_ text: String) -> some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            PromptBubble(text: text)
            HStack(spacing: Design.Space.l) {
                SpeakingIndicator()
                if let status = model.status {
                    Text(status)
                        .font(Design.label)
                        .foregroundStyle(Design.inkFaint)
                }
            }
            .responseShell()
        }
    }

    private var emptyTranscript: some View {
        Text(
            boundRecord.map {
                "Give \($0.displayName) something to say — every take lands here, playable and downloadable."
            } ?? "When a voice model lands on your shelf, it speaks from here."
        )
        .font(Design.caption)
        .foregroundStyle(Design.inkFaint)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var placeholder: String {
        boundRecord.map { "What should \($0.displayName) say?" } ?? "What should your Mac say?"
    }

    private var speakable: Bool {
        !model.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && model.boundModelID != nil
    }

    private var modelChip: some View {
        InkMenu(
            title: boundRecord?.displayName ?? "Choose model",
            accessibilityName: "Voice model"
        ) {
            let runnable = model.runnableModels(in: shell.library.records)
            if runnable.isEmpty {
                InkMenuRow(title: "No voice model is ready.", disabled: true) {}
            }
            ForEach(runnable) { record in
                InkMenuRow(
                    title: record.displayName,
                    selected: record.id == model.boundModelID
                ) {
                    let shell = shell
                    Task {
                        await shell.voice.bind(to: record)
                        shell.selectVoice(record.id)
                    }
                }
            }
        }
        .disabled(model.isSpeaking)
    }

    private var voiceChip: some View {
        InkMenu(title: model.voice, accessibilityName: "Voice") {
            ForEach(model.voices, id: \.self) { candidate in
                InkMenuRow(title: candidate, selected: candidate == model.voice) {
                    model.voice = candidate
                }
            }
        }
        .disabled(model.voices.isEmpty || model.isSpeaking)
    }
}
