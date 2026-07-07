import AVFoundation
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
final class AudioClipController {
    private var player: AVAudioPlayer?
    private var ticker: Task<Void, Never>?
    private let relay = ClipFinishRelay()

    var playingID: String?
    var isPaused = false
    var progress: Double = 0
    var elapsed: TimeInterval = 0

    func isActive(_ id: String) -> Bool {
        playingID == id
    }

    func isSounding(_ id: String) -> Bool {
        playingID == id && !isPaused
    }

    func toggle(id: String, url: URL? = nil) {
        if playingID == id, let player {
            if isPaused {
                player.play()
                isPaused = false
                startTicker()
            } else {
                player.pause()
                isPaused = true
                ticker?.cancel()
            }
            return
        }
        stop()
        guard let url, let fresh = try? AVAudioPlayer(contentsOf: url) else { return }
        relay.onFinish = { [weak self] in
            self?.stop()
        }
        fresh.delegate = relay
        player = fresh
        playingID = id
        isPaused = false
        fresh.play()
        startTicker()
    }

    func seek(to fraction: Double) {
        guard let player, playingID != nil else { return }
        let clamped = min(max(fraction, 0), 1)
        player.currentTime = clamped * player.duration
        refresh()
    }

    func stop() {
        ticker?.cancel()
        ticker = nil
        player?.stop()
        player = nil
        playingID = nil
        isPaused = false
        progress = 0
        elapsed = 0
    }

    private func startTicker() {
        ticker?.cancel()
        ticker = Task { [weak self] in
            while !Task.isCancelled {
                guard let self, self.player != nil else { return }
                self.refresh()
                try? await Task.sleep(for: .milliseconds(33))
            }
        }
    }

    private func refresh() {
        guard let player else { return }
        elapsed = player.currentTime
        progress = player.duration > 0 ? player.currentTime / player.duration : 0
    }
}
