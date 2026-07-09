import AVFoundation
import AppKit
import HedosKernel

@MainActor
final class PCMPlayer {
    private var engine: AVAudioEngine?
    private var player: AVAudioPlayerNode?
    private var format: AVAudioFormat?
    private var scheduledFrames: AVAudioFramePosition = 0

    var scheduledSeconds: TimeInterval {
        guard let format, scheduledFrames > 0 else { return 0 }
        return Double(scheduledFrames) / format.sampleRate
    }

    var isDrained: Bool {
        guard scheduledFrames > 0 else { return true }
        guard let player, player.isPlaying else { return player == nil }
        guard let nodeTime = player.lastRenderTime,
            let playerTime = player.playerTime(forNodeTime: nodeTime)
        else { return false }
        return playerTime.sampleTime >= scheduledFrames
    }

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
            scheduledFrames = 0
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
        scheduledFrames += AVAudioFramePosition(buffer.frameLength)
        if !player.isPlaying {
            player.play()
        }
    }

    func stop() {
        scheduledFrames = 0
        player?.stop()
        engine?.stop()
        player = nil
        engine = nil
        format = nil
    }
}

enum SpeechModels {
    static let previewLine = "Hello, World!"

    static func speakers(in records: [ModelRecord]) -> [ModelRecord] {
        records.filter { $0.state == .ready && Launcher.destination(for: $0) == .voice }
    }

    static func preferred(in records: [ModelRecord]) -> ModelRecord? {
        let candidates = speakers(in: records)
        let voiced = candidates.map { ($0, SpeechVoices.available($0).count) }.filter { $0.1 > 0 }
        return voiced.max { $0.1 < $1.1 }?.0 ?? candidates.first
    }
}

enum SpeechArtifact {
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
}
