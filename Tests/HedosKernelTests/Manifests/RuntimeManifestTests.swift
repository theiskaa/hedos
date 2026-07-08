import Foundation
import Testing

@testable import HedosKernel

private let validInvokeManifest = """
    id           = "kokoro-cli"
    modalities   = ["speech"]
    capabilities = ["speak"]
    execution    = "stream"
    detect       = { extension = "pth" }

    [invoke]
    command = "kokoro-tool --model {model} --text {prompt}"

    [permissions]
    network = false
    paths   = ["{model}", "{workdir}"]
    """

@Test func loadsValidInvokeManifest() throws {
    let table = try TOMLLite.parse(validInvokeManifest)
    let manifest = try RuntimeManifest.load(table: table, directory: nil)
    #expect(manifest.id == "kokoro-cli")
    #expect(manifest.modalities == [.speech])
    #expect(manifest.capabilities == [.speak])
    #expect(manifest.execution == .stream)
    #expect(manifest.detect?.fileExtension == "pth")
    #expect(manifest.invoke?.command.contains("{prompt}") == true)
    #expect(manifest.permissions.network == false)
}

@Test func parsesSection18ExampleAndNamesItsValidationGap() throws {
    let section18 = """
        id           = "python:mflux-user"
        modalities   = ["image"]
        capabilities = ["image"]
        execution    = "job"
        detect       = { file = "model_index.json", contains = "FluxPipeline" }

        [env]
        manager  = "uv"
        python   = "3.12"
        packages = ["mflux"]
        lockfile = "mflux.lock"        # exact pins ship with the manifest;
                                       # pin updates are data updates, not app releases

        [serve]
        command  = "hedos-runtime-image --model {path} --port {port}"
        protocol = "ndjson+frames"

        [permissions]
        network = false
        paths   = ["{model}", "{workdir}", "{outputs}"]
        """
    let table = try TOMLLite.parse(section18)
    #expect(table["detect"]?.tableValue?["contains"]?.stringValue == "FluxPipeline")
    do {
        _ = try RuntimeManifest.load(table: table, directory: nil)
        Issue.record("expected the entrypoint validation gap")
    } catch let error as ManifestValidationError {
        #expect(error.message.contains("entrypoint"))
    }
}

@Test func loadsAllFiveBundledManifests() throws {
    for name in [
        "python-mlx-audio", "python-mflux", "python-diffusers", "python-mlx-lm",
        "python-whisper-cpp",
    ] {
        let bundle = try #require(RuntimeBundle.directory(named: name))
        let text = try String(
            contentsOf: bundle.appendingPathComponent("manifest.toml"), encoding: .utf8)
        let manifest = try RuntimeManifest.load(
            table: try TOMLLite.parse(text), directory: bundle)
        #expect(!manifest.id.isEmpty)
        #expect(!manifest.capabilities.isEmpty)
        #expect(manifest.serve != nil)
        #expect(manifest.env != nil)
    }
}

@Test func rejectionMatrix() throws {
    let missingID = "capabilities = [\"chat\"]\nexecution = \"stream\"\n[invoke]\ncommand = \"x\""
    #expect(throws: ManifestValidationError.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(missingID), directory: nil)
    }

    let noCapabilities =
        "id = \"a\"\ncapabilities = []\nexecution = \"stream\"\n[invoke]\ncommand = \"x\""
    #expect(throws: ManifestValidationError.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(noCapabilities), directory: nil)
    }

    let badExecution =
        "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"warp\"\n[invoke]\ncommand = \"x\""
    #expect(throws: ManifestValidationError.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(badExecution), directory: nil)
    }

    let neither = "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"stream\""
    #expect(throws: ManifestValidationError.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(neither), directory: nil)
    }

    let both = """
        id = "a"
        capabilities = ["chat"]
        execution = "stream"
        [serve]
        entrypoint = "main.py"
        [invoke]
        command = "x"
        """
    #expect(throws: ManifestValidationError.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(both), directory: nil)
    }
}

@Test func detectPredicateMatrix() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let bundle = dir.appendingPathComponent("model")
    try FileManager.default.createDirectory(at: bundle, withIntermediateDirectories: true)
    try Data(#"{"_class_name": "SomePipeline"}"#.utf8)
        .write(to: bundle.appendingPathComponent("model_index.json"))
    var record = ModelRecord(
        name: "model", modality: .unknown, capabilities: [],
        source: ModelSource(kind: .folder, path: bundle.path))
    record.primaryWeightPath = bundle.appendingPathComponent("weights.xyz").path

    #expect(ManifestDetect(file: "model_index.json").matches(record))
    #expect(!ManifestDetect(file: "config.json").matches(record))
    #expect(ManifestDetect(file: "model_index.json", contains: "SomePipeline").matches(record))
    #expect(!ManifestDetect(file: "model_index.json", contains: "Other").matches(record))
    #expect(ManifestDetect(fileExtension: "xyz").matches(record))
    #expect(ManifestDetect(fileExtension: "XYZ").matches(record))
    #expect(!ManifestDetect(fileExtension: "gguf").matches(record))
    #expect(!ManifestDetect().matches(record))
}
