@preconcurrency import AVFoundation
import HedosKernel
import SwiftUI

@Observable
@MainActor
final class VoiceConversationController {
    private(set) var active = false
    private(set) var status: String?
    var notice: String?

    private static let trackID = "voice-conversation"

    private let capture = MicCapture()
    private var audio: AudioSession?
    private var loop: VoiceLoop?
    private var eventTask: Task<Void, Never>?
    private var feedTask: Task<Void, Never>?

    static func participants(
        in records: [ModelRecord]
    ) -> (transcriber: ModelRecord, speaker: ModelRecord)? {
        guard
            let transcriber = records.first(where: {
                $0.state == .ready && $0.capabilities.contains(.transcribe)
            }),
            let speaker = records.first(where: {
                $0.state == .ready && Launcher.destination(for: $0) == .voice
            })
        else { return nil }
        return (transcriber, speaker)
    }

    func toggle(
        sessionID: String, kernel: Kernel, records: [ModelRecord], audio: AudioSession,
        onTurn: @escaping () -> Void
    ) {
        if active {
            stop()
            return
        }
        self.audio = audio
        start(sessionID: sessionID, kernel: kernel, records: records, onTurn: onTurn)
    }

    func stop() {
        active = false
        status = nil
        eventTask?.cancel()
        eventTask = nil
        feedTask?.cancel()
        feedTask = nil
        capture.stop()
        if audio?.isActive(Self.trackID) == true {
            audio?.dismiss()
        }
        let loop = loop
        self.loop = nil
        Task {
            await loop?.stop()
        }
    }

    private func start(
        sessionID: String, kernel: Kernel, records: [ModelRecord],
        onTurn: @escaping () -> Void
    ) {
        guard let participants = Self.participants(in: records) else {
            notice = "A voice conversation needs a ready transcription model and a ready voice model."
            return
        }
        notice = nil
        Task { [weak self] in
            let granted = await AVCaptureDevice.requestAccess(for: .audio)
            guard let self else { return }
            guard granted else {
                notice = "Hedos needs microphone access for voice conversations."
                return
            }
            await begin(
                sessionID: sessionID, kernel: kernel, participants: participants,
                onTurn: onTurn)
        }
    }

    private func begin(
        sessionID: String, kernel: Kernel,
        participants: (transcriber: ModelRecord, speaker: ModelRecord),
        onTurn: @escaping () -> Void
    ) async {
        let voices = (try? await kernel.voices(for: participants.speaker.id)) ?? []
        var voice = voices.first ?? SpeechVoices.fallback
        if case .string(let configured)? = participants.speaker.paramValues["voice"],
            voices.contains(configured)
        {
            voice = configured
        } else if let fallback = await kernel.settings.voice().defaultVoice,
            voices.contains(fallback)
        {
            voice = fallback
        }

        let speed: Double
        if case .double(let saved)? = participants.speaker.paramValues["speed"] {
            speed = saved
        } else {
            speed = await kernel.settings.voice().speed
        }

        let loop = await kernel.voiceLoop(
            sessionID: sessionID,
            transcriberID: participants.transcriber.id,
            speakerID: participants.speaker.id,
            voice: voice,
            speed: speed)
        self.loop = loop

        audio?.beginLive(
            AudioSession.Track(
                id: Self.trackID, title: "Voice conversation", subtitle: voice),
            audible: true,
            onStop: { [weak self] in self?.stop() })

        let events = await loop.start()
        let (samples, feed) = AsyncStream.makeStream(of: [Float].self)
        do {
            try capture.start { chunk in
                feed.yield(chunk)
            }
        } catch {
            notice = error.localizedDescription
            await loop.stop()
            self.loop = nil
            return
        }
        active = true
        status = "Listening…"

        feedTask = Task {
            for await chunk in samples {
                await loop.feed(chunk)
            }
        }
        eventTask = Task { [weak self] in
            for await event in events {
                guard let self else { return }
                switch event {
                case .listening:
                    status = "Listening…"
                case .userSpeechBegan:
                    audio?.flushLive(Self.trackID)
                    status = "Hearing you…"
                case .userTurn(let text):
                    status = "Heard: \(text)"
                case .assistantDelta:
                    break
                case .speech(let frame):
                    status = "Speaking…"
                    audio?.enqueue(frame, for: Self.trackID)
                case .status(let message):
                    status = message.capitalized + "…"
                case .turnCompleted:
                    onTurn()
                case .failed(let message):
                    notice = message
                }
            }
        }
    }
}
