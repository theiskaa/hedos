import Foundation

@testable import HedosKernel

enum Fixtures {
    static func flux() -> ModelRecord {
        ModelRecord(
            name: "FLUX.1-schnell",
            modality: .image,
            capabilities: [.image],
            source: ModelSource(
                kind: .huggingfaceCache,
                path: "~/.cache/huggingface/hub/models--black-forest-labs--FLUX.1-schnell",
                repo: "black-forest-labs/FLUX.1-schnell"),
            runtime: RuntimeRef(
                id: "python:mflux",
                resolved: .auto,
                tier: .managed,
                alternatives: ["python:diffusers"]),
            params: [
                ParamSpec(key: "steps", type: .int, defaultValue: .int(4), range: [.int(1), .int(50)]),
                ParamSpec(
                    key: "guidance", type: .float, defaultValue: .double(0.0),
                    range: [.double(0), .double(10)]),
                ParamSpec(
                    key: "size", type: .enumeration, defaultValue: .string("1024x1024"),
                    values: ["512x512", "768x768", "1024x1024"]),
                ParamSpec(key: "seed", type: .int),
            ],
            execution: .job,
            footprintMB: 34000,
            state: .ready,
            registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
    }

    static func gguf(path: String = "~/Downloads/qwen3.5-9b-q4.gguf") -> ModelRecord {
        ModelRecord(
            name: "qwen3.5-9b-q4",
            modality: .text,
            capabilities: [.chat, .complete],
            source: ModelSource(kind: .file, path: path),
            execution: .stream,
            registeredAt: Date(timeIntervalSince1970: 1_750_000_000))
    }

    static func tempDirectory() throws -> URL {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("hedos-tests-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        return url
    }
}
