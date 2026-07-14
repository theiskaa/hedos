import ArgumentParser
import Foundation
import HedosKernel

struct Speak: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "speak",
        abstract: "Generate speech from text and write it to a .wav file.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "Model id or name.")
    var model: String

    @Argument(help: "The text to speak.")
    var text: String

    @Option(name: .long, help: "Voice to use (defaults to the model's first voice).")
    var voice: String?

    @Option(name: .long, help: "Speaking speed multiplier.")
    var speed: Double = 1.0

    @Option(name: [.customShort("o"), .long], help: "Output file path (.wav).")
    var output: String?

    func run() async throws {
        let kernel = Session.kernel()
        let shelf = try await Session.shelf(kernel)
        let record = try Session.resolve(model, in: shelf, capability: .speak)

        let voices = (try? await kernel.voices(for: record.id)) ?? []
        let chosenVoice = voice ?? voices.first ?? "default"

        let payload: JSONValue = .object([
            "text": .string(text),
            "voice": .string(chosenVoice),
            "speed": .double(speed),
        ])

        var pcm = Data()
        var sampleRate = 24000
        for try await chunk in try await kernel.invoke(record.id, .speak, payload: payload) {
            switch chunk {
            case .audio(let frame):
                pcm.append(frame.data)
                sampleRate = frame.sampleRate
            case .status(let status):
                if !global.json { Out.err(status) }
            default:
                break
            }
        }
        guard !pcm.isEmpty else { throw CLIError("\(record.displayName) produced no audio.") }

        let artifact = try await kernel.saveSpeech(
            modelID: record.id, voice: chosenVoice, text: text,
            speed: speed, sampleRate: sampleRate, pcm: pcm)
        let stored = try await kernel.artifactStore.url(id: artifact.id)
        let finalPath = try deliver(stored, to: output)

        if global.json {
            try Out.json(MediaReport(
                model: record.id, path: finalPath, voice: chosenVoice,
                durationMs: artifact.durationMs))
        } else {
            Out.line(finalPath)
        }
    }
}

func deliver(_ stored: URL?, to output: String?) throws -> String {
    guard let stored else { throw CLIError("the output was not written to disk.") }
    guard let output else { return stored.path }
    let destination = URL(fileURLWithPath: (output as NSString).expandingTildeInPath)
    var isDirectory: ObjCBool = false
    if FileManager.default.fileExists(atPath: destination.path, isDirectory: &isDirectory),
        isDirectory.boolValue
    {
        throw CLIError("\(destination.path) is a directory — pass a file path to -o.")
    }
    try? FileManager.default.removeItem(at: destination)
    try FileManager.default.copyItem(at: stored, to: destination)
    return destination.path
}

struct MediaReport: Encodable {
    let model: String
    let path: String
    var voice: String? = nil
    var durationMs: Int? = nil
}
