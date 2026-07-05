import AVFoundation
import HedosKernel
import SwiftUI

@MainActor
final class PCMPlayer {
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private var format: AVAudioFormat?

    func enqueue(_ frame: AudioFrame) {
        if format == nil || format?.sampleRate != Double(frame.sampleRate) {
            let newFormat = AVAudioFormat(
                commonFormat: .pcmFormatFloat32,
                sampleRate: Double(frame.sampleRate),
                channels: 1,
                interleaved: false)!
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
        player.stop()
        engine.stop()
        format = nil
    }
}

@Observable
@MainActor
final class VoiceViewModel {
    private let kernel: Kernel
    private let modelID: String
    private var speakTask: Task<Void, Never>?
    private let player = PCMPlayer()

    var text = "Hedos gives every local model a home."
    var voice = "af_heart"
    var voices: [String] = []
    var status: String?
    var notice: String?
    var isSpeaking = false
    var isPlaying = false

    init(kernel: Kernel, modelID: String) {
        self.kernel = kernel
        self.modelID = modelID
    }

    func loadVoices() async {
        voices = (try? await kernel.voices(modelID)) ?? []
        if !voices.contains(voice), let first = voices.first {
            voice = first
        }
    }

    func speak() {
        let content = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !content.isEmpty, !isSpeaking else { return }
        notice = nil
        isSpeaking = true
        speakTask = Task { [weak self] in
            guard let self else { return }
            do {
                let stream = try await kernel.invoke(
                    modelID, .speak,
                    payload: .object([
                        "text": .string(content),
                        "voice": .string(voice),
                    ]))
                for try await chunk in stream {
                    switch chunk {
                    case .status(let message):
                        status = message
                    case .audio(let frame):
                        status = nil
                        isPlaying = true
                        player.enqueue(frame)
                    case .done:
                        break
                    default:
                        break
                    }
                }
            } catch is CancellationError {
            } catch {
                notice = error.localizedDescription
            }
            status = nil
            isSpeaking = false
            isPlaying = false
        }
    }

    func stop() {
        speakTask?.cancel()
        player.stop()
        status = nil
        isSpeaking = false
        isPlaying = false
    }
}

struct VoiceView: View {
    let record: ModelRecord
    @State private var model: VoiceViewModel

    init(record: ModelRecord, kernel: Kernel) {
        self.record = record
        _model = State(initialValue: VoiceViewModel(kernel: kernel, modelID: record.id))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Text(record.name)
                    .font(.system(size: 15, weight: .semibold))
                Spacer()
                if model.isPlaying {
                    SpeakingIndicator()
                }
            }

            TextEditor(text: $model.text)
                .font(.system(size: 13))
                .lineSpacing(3)
                .scrollContentBackground(.hidden)
                .frame(minHeight: 100, maxHeight: 180)
                .hedosField()

            HStack(spacing: 12) {
                Picker("Voice", selection: $model.voice) {
                    ForEach(model.voices, id: \.self) { voice in
                        Text(voice).tag(voice)
                    }
                }
                .labelsHidden()
                .frame(maxWidth: 170)
                .disabled(model.voices.isEmpty)

                Spacer()

                if model.isSpeaking {
                    Button("Stop") {
                        model.stop()
                    }
                } else {
                    Button("Speak") {
                        model.speak()
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(Design.accent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(
                        model.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }

            if let status = model.status {
                HStack(spacing: 7) {
                    ProgressView().controlSize(.mini)
                    Text(status)
                        .font(.system(size: 12))
                        .foregroundStyle(.secondary)
                }
            }
            if let notice = model.notice {
                Text(notice)
                    .font(.system(size: 12))
                    .foregroundStyle(Design.warn)
            }
            Spacer()
        }
        .padding(24)
        .frame(maxWidth: 560, alignment: .topLeading)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .navigationTitle(record.name)
        .navigationSubtitle(record.runtime.id ?? "")
        .task { await model.loadVoices() }
    }
}
