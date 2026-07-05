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
    var status: String?
    var notice: String?
    var isSpeaking = false

    init(kernel: Kernel, modelID: String) {
        self.kernel = kernel
        self.modelID = modelID
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
                        player.enqueue(frame)
                    case .done:
                        break
                    default:
                        break
                    }
                }
            } catch KernelError.runtimeUnavailable(let hint) {
                notice = hint
            } catch is CancellationError {
            } catch {
                notice = "Speech failed: \(error.localizedDescription)"
            }
            status = nil
            isSpeaking = false
        }
    }

    func stop() {
        speakTask?.cancel()
        player.stop()
        status = nil
        isSpeaking = false
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
            Text(record.name).font(.title2.weight(.semibold))
            TextEditor(text: $model.text)
                .font(.body)
                .scrollContentBackground(.hidden)
                .padding(8)
                .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 8))
                .frame(minHeight: 100, maxHeight: 180)

            HStack(spacing: 12) {
                Picker("Voice", selection: $model.voice) {
                    Text("af_heart").tag("af_heart")
                    Text("af_bella").tag("af_bella")
                    Text("am_michael").tag("am_michael")
                    Text("bf_emma").tag("bf_emma")
                }
                .frame(maxWidth: 220)

                Spacer()

                if model.isSpeaking {
                    Button {
                        model.stop()
                    } label: {
                        Label("Stop", systemImage: "stop.fill")
                    }
                } else {
                    Button {
                        model.speak()
                    } label: {
                        Label("Speak", systemImage: "waveform")
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(
                        model.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }

            if let status = model.status {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(status).foregroundStyle(.secondary)
                }
            }
            if let notice = model.notice {
                Label(notice, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
                    .font(.callout)
            }
            Spacer()
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}
