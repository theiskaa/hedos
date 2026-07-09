import AVFoundation
import AppKit
import HedosKernel
import SwiftUI

@MainActor
private final class ClipFinishRelay: NSObject, AVAudioPlayerDelegate {
    var onFinish: (() -> Void)?

    nonisolated func audioPlayerDidFinishPlaying(
        _ player: AVAudioPlayer, successfully flag: Bool
    ) {
        Task { @MainActor in
            self.onFinish?()
        }
    }
}

@Observable
@MainActor
final class AudioSession {
    struct Track: Equatable {
        var id: String
        var title: String
        var subtitle: String?
        var peaks: [Double] = []
        var durationMs: Int = 0
        var artifactID: String?
    }

    enum Phase: Equatable {
        case idle
        case live
        case clip
    }

    static let rates: [Float] = [1.0, 1.25, 1.5, 2.0]

    private let kernel: Kernel
    private let live = PCMPlayer()
    private let relay = ClipFinishRelay()
    private var player: AVAudioPlayer?
    private var ticker: Task<Void, Never>?
    private var loadTask: Task<Void, Never>?
    private var drainTask: Task<Void, Never>?
    private var spaceMonitor: Any?
    private var liveAudible = true
    private var liveStop: (() -> Void)?
    private var positions: [String: TimeInterval] = [:]

    private(set) var track: Track?
    private(set) var phase: Phase = .idle
    private(set) var isPaused = false
    private(set) var progress: Double = 0
    private(set) var elapsed: TimeInterval = 0
    private(set) var rate: Float = 1.0

    init(kernel: Kernel) {
        self.kernel = kernel
    }

    static func track(for artifact: Artifact) -> Track {
        Track(
            id: artifact.id,
            title: SpeechArtifact.text(of: artifact),
            subtitle: SpeechArtifact.voiceName(of: artifact),
            peaks: SpeechArtifact.peaks(of: artifact),
            durationMs: artifact.durationMs,
            artifactID: artifact.id)
    }

    func isActive(_ id: String) -> Bool {
        track?.id == id
    }

    func isSounding(_ id: String) -> Bool {
        isActive(id) && phase != .idle && !isPaused
    }

    var duration: TimeInterval {
        guard let track else { return 0 }
        return Double(max(1000, track.durationMs)) / 1000
    }

    func toggle(_ artifact: Artifact) {
        if isActive(artifact.id), phase == .clip {
            togglePlayback()
            return
        }
        play(artifact)
    }

    func play(_ artifact: Artifact) {
        stopEngines()
        track = Self.track(for: artifact)
        phase = .clip
        isPaused = true
        progress = 0
        elapsed = 0
        installSpaceMonitor()
        resume(artifactID: artifact.id)
    }

    func togglePlayback() {
        switch phase {
        case .live:
            dismiss()
        case .clip:
            if let player {
                if isPaused {
                    player.play()
                    isPaused = false
                    startTicker()
                } else {
                    player.pause()
                    isPaused = true
                    ticker?.cancel()
                    ticker = nil
                }
            } else if let artifactID = track?.artifactID {
                resume(artifactID: artifactID)
            }
        case .idle:
            break
        }
    }

    func beginLive(_ track: Track, audible: Bool, onStop: (() -> Void)? = nil) {
        let superseded = phase == .live ? liveStop : nil
        liveStop = nil
        stopEngines()
        self.track = track
        phase = .live
        liveAudible = audible
        liveStop = onStop
        isPaused = false
        progress = 0
        elapsed = 0
        installSpaceMonitor()
        superseded?()
    }

    func enqueue(_ frame: AudioFrame, for id: String) {
        guard phase == .live, liveAudible, track?.id == id else { return }
        live.enqueue(frame)
    }

    func flushLive(_ id: String) {
        guard phase == .live, track?.id == id else { return }
        live.stop()
    }

    func finishLive(_ id: String) {
        guard phase == .live, track?.id == id else { return }
        liveStop = nil
        guard liveAudible else {
            dismiss()
            return
        }
        drainTask?.cancel()
        drainTask = Task { [weak self] in
            guard let self else { return }
            let deadline = live.scheduledSeconds + 1
            var waited: TimeInterval = 0
            while phase == .live, track?.id == id, !Task.isCancelled,
                !live.isDrained, waited < deadline
            {
                try? await Task.sleep(for: .milliseconds(50))
                waited += 0.05
            }
            guard phase == .live, track?.id == id, !Task.isCancelled else { return }
            dismiss()
        }
    }

    func stop() {
        dismiss()
    }

    func dismissIfActive(_ id: String) {
        guard track?.id == id else { return }
        dismiss()
    }

    func dismiss() {
        let onStop = phase == .live ? liveStop : nil
        liveStop = nil
        stopEngines()
        track = nil
        phase = .idle
        isPaused = false
        progress = 0
        elapsed = 0
        removeSpaceMonitor()
        onStop?()
    }

    func seek(to fraction: Double) {
        guard phase == .clip, let player else { return }
        let clamped = min(max(fraction, 0), 1)
        player.currentTime = clamped * player.duration
        refresh()
    }

    func setRate(_ newRate: Float) {
        rate = newRate
        player?.rate = newRate
    }

    func cycleRate() {
        let index = Self.rates.firstIndex(of: rate) ?? 0
        setRate(Self.rates[(index + 1) % Self.rates.count])
    }

    var rateLabel: String {
        rate == rate.rounded() ? "\(Int(rate))×" : String(format: "%g×", rate)
    }

    private func resume(artifactID: String) {
        loadTask?.cancel()
        let kernel = kernel
        loadTask = Task { [weak self] in
            guard let url = try? await kernel.artifactURL(id: artifactID) else { return }
            guard let self, track?.artifactID == artifactID, !Task.isCancelled else { return }
            startClip(url: url, resumeAt: positions[artifactID] ?? 0)
        }
    }

    private func startClip(url: URL, resumeAt: TimeInterval) {
        guard let fresh = try? AVAudioPlayer(contentsOf: url) else { return }
        relay.onFinish = { [weak self] in
            self?.finishClip()
        }
        fresh.delegate = relay
        fresh.enableRate = true
        fresh.rate = rate
        if resumeAt > 0, resumeAt < fresh.duration - 0.15 {
            fresh.currentTime = resumeAt
        }
        player = fresh
        isPaused = false
        fresh.play()
        startTicker()
        refresh()
    }

    private func finishClip() {
        let finished = track?.id
        player?.stop()
        player = nil
        if let finished {
            positions[finished] = 0
        }
        dismiss()
    }

    private func stopEngines() {
        ticker?.cancel()
        ticker = nil
        loadTask?.cancel()
        loadTask = nil
        drainTask?.cancel()
        drainTask = nil
        if phase == .clip, let id = track?.id, let player {
            positions[id] = player.isPlaying || player.currentTime > 0 ? player.currentTime : 0
        }
        player?.stop()
        player = nil
        live.stop()
    }

    private func startTicker() {
        ticker?.cancel()
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, self.player != nil else { return }
                self.refresh()
                try? await Task.sleep(for: .milliseconds(40))
            }
        }
    }

    private func refresh() {
        guard let player else { return }
        let time = player.currentTime
        let fraction = player.duration > 0 ? time / player.duration : 0
        if abs(time - elapsed) >= 0.02 {
            elapsed = time
        }
        if abs(fraction - progress) >= 0.001 {
            progress = fraction
        }
    }

    private func installSpaceMonitor() {
        guard spaceMonitor == nil else { return }
        spaceMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self, self.track != nil,
                event.charactersIgnoringModifiers == " ",
                event.modifierFlags.intersection([.command, .option, .control]).isEmpty,
                !(NSApp.keyWindow?.firstResponder is NSTextView)
            else { return event }
            self.togglePlayback()
            return nil
        }
    }

    private func removeSpaceMonitor() {
        if let spaceMonitor {
            NSEvent.removeMonitor(spaceMonitor)
            self.spaceMonitor = nil
        }
    }
}
