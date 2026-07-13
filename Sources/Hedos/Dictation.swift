@preconcurrency import AVFoundation
import HedosKernel
import SwiftUI

private final class SampleSink: @unchecked Sendable {
    static let maxSamples = WhisperEngine.expectedSampleRate * 60 * 10

    private let lock = NSLock()
    private var storage: [Float] = []
    private var overflowed = false

    func append(_ new: [Float]) {
        lock.lock()
        let room = Self.maxSamples - storage.count
        if room >= new.count {
            storage.append(contentsOf: new)
        } else {
            if room > 0 { storage.append(contentsOf: new.prefix(room)) }
            overflowed = true
        }
        lock.unlock()
    }

    func drain() -> (samples: [Float], overflowed: Bool) {
        lock.lock()
        defer {
            storage = []
            overflowed = false
            lock.unlock()
        }
        return (storage, overflowed)
    }
}

struct DictationSetup {
    let kernel: Kernel
    let records: () -> [ModelRecord]
}

@MainActor
final class MicCapture {
    private var engine: AVAudioEngine?

    var isRunning: Bool {
        engine != nil
    }

    func start(onSamples: @escaping @Sendable ([Float]) -> Void) throws {
        guard engine == nil else { return }
        let engine = AVAudioEngine()
        let input = engine.inputNode
        let inputFormat = input.outputFormat(forBus: 0)
        guard inputFormat.sampleRate > 0,
            let target = AVAudioFormat(
                commonFormat: .pcmFormatFloat32,
                sampleRate: Double(WhisperEngine.expectedSampleRate),
                channels: 1,
                interleaved: false),
            let converter = AVAudioConverter(from: inputFormat, to: target)
        else {
            throw KernelError.runtimeFailed("No usable microphone was found.")
        }
        let ratio = target.sampleRate / inputFormat.sampleRate
        input.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) { buffer, _ in
            let capacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio) + 32
            guard let out = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity) else {
                return
            }
            nonisolated(unsafe) var fed = false
            converter.convert(to: out, error: nil) { _, status in
                if fed {
                    status.pointee = .noDataNow
                    return nil
                }
                fed = true
                status.pointee = .haveData
                return buffer
            }
            guard out.frameLength > 0, let channel = out.floatChannelData else { return }
            onSamples(Array(UnsafeBufferPointer(start: channel[0], count: Int(out.frameLength))))
        }
        do {
            try engine.start()
            self.engine = engine
        } catch {
            input.removeTap(onBus: 0)
            throw error
        }
    }

    func stop() {
        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil
    }
}

@Observable
@MainActor
final class DictationController {
    enum Phase {
        case idle
        case recording
        case transcribing
    }

    private(set) var phase: Phase = .idle
    var notice: String?

    private let capture = MicCapture()
    private let sink = SampleSink()
    private var task: Task<Void, Never>?
    private var generation = 0

    static func transcriber(in records: [ModelRecord]) -> ModelRecord? {
        records.first { $0.state == .ready && $0.capabilities.contains(.transcribe) }
    }

    func toggle(setup: DictationSetup, append: @escaping (String) -> Void) {
        switch phase {
        case .recording:
            finishRecording(setup: setup, append: append)
        case .transcribing:
            task?.cancel()
            task = nil
            generation += 1
            phase = .idle
        case .idle:
            startRecording()
        }
    }

    private func startRecording() {
        notice = nil
        Task { [weak self] in
            let granted = await AVCaptureDevice.requestAccess(for: .audio)
            guard let self else { return }
            guard granted else {
                notice = "Hedos needs microphone access to dictate. Grant it in System Settings."
                return
            }
            beginCapture()
        }
    }

    private func beginCapture() {
        guard phase == .idle else { return }
        let sink = sink
        do {
            try capture.start { chunk in
                sink.append(chunk)
            }
            _ = sink.drain()
            generation += 1
            phase = .recording
        } catch {
            notice = "The microphone could not be started: \(error.localizedDescription)"
        }
    }

    private func finishRecording(setup: DictationSetup, append: @escaping (String) -> Void) {
        capture.stop()
        let (samples, overflowed) = sink.drain()
        guard let transcriber = Self.transcriber(in: setup.records()) else {
            generation += 1
            phase = .idle
            notice = "No transcription model is ready."
            return
        }
        guard samples.count >= WhisperEngine.expectedSampleRate / 4 else {
            generation += 1
            phase = .idle
            return
        }
        if overflowed {
            notice = "Dictation is capped at 10 minutes; the recording was trimmed."
        }
        generation += 1
        phase = .transcribing
        let current = generation
        let kernel = setup.kernel
        task = Task { [weak self] in
            do {
                let base64 = samples.withUnsafeBytes { Data($0) }.base64EncodedString()
                let stream = try await kernel.invoke(
                    transcriber.id, .transcribe,
                    payload: .object([
                        "pcm": .string(base64),
                        "sampleRate": .int(WhisperEngine.expectedSampleRate),
                    ]))
                for try await chunk in stream {
                    switch chunk {
                    case .text(let delta), .segment(let delta, _, _):
                        append(delta)
                    default:
                        break
                    }
                }
            } catch is CancellationError {
            } catch {
                self?.notice = error.localizedDescription
            }
            guard let self, self.generation == current else { return }
            self.task = nil
            self.phase = .idle
        }
    }
}
