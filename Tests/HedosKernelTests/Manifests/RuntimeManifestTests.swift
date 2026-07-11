import Foundation
import Testing

@testable import HedosKernel

private let validInvokeManifest = """
    id           = "kokoro-cli"
    modalities   = ["speech"]
    capabilities = ["speak"]
    execution    = "sync"
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
    #expect(manifest.execution == .sync)
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

    let chatAsJob = """
        id = "a"
        capabilities = ["chat"]
        execution = "job"
        [invoke]
        command = "x"
        """
    #expect(throws: ManifestValidationError.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(chatAsJob), directory: nil)
    }

    let imageAsStream = """
        id = "a"
        capabilities = ["image"]
        execution = "stream"
        [invoke]
        command = "x"
        """
    #expect(throws: ManifestValidationError.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(imageAsStream), directory: nil)
    }

    let imageJob = """
        id = "a"
        capabilities = ["image"]
        execution = "job"
        [invoke]
        command = "x"
        """
    #expect(throws: Never.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(imageJob), directory: nil)
    }
}

@Test func vmManifestRejectsNetworkPermission() throws {
    let base = """
        id = "python:kokoro-vm"
        capabilities = ["speak"]
        execution = "sync"
        detect = { extension = "pth" }
        [invoke]
        command = "kokoro {prompt}"
        [vm]
        image = "ghcr.io/acme/kokoro@sha256:abc123"
        """
    #expect(throws: Never.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(base), directory: nil)
    }
    let networked = base + "\n[permissions]\nnetwork = true"
    do {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(networked), directory: nil)
        Issue.record("a [vm] manifest asking for network should be rejected")
    } catch let error as ManifestValidationError {
        #expect(error.message.contains("vm runtimes always run offline"))
    }
    let offline = base + "\n[permissions]\nnetwork = false"
    #expect(throws: Never.self) {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(offline), directory: nil)
    }
}

@Test func invokeStreamIsRejectedButSyncJobServeAreAccepted() throws {
    let invokeStream = """
        id = "a"
        capabilities = ["chat"]
        execution = "stream"
        [invoke]
        command = "x"
        """
    do {
        _ = try RuntimeManifest.load(table: try TOMLLite.parse(invokeStream), directory: nil)
        Issue.record("invoke + stream should be rejected")
    } catch let error as ManifestValidationError {
        #expect(error.message.contains("invoke manifests run to completion"))
    }

    let invokeSync = "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"x\""
    let invokeJob = "id = \"a\"\ncapabilities = [\"image\"]\nexecution = \"job\"\n[invoke]\ncommand = \"x\""
    let serveStream = "id = \"a\"\ncapabilities = [\"chat\"]\nexecution = \"stream\"\n[serve]\nentrypoint = \"m.py\""
    for good in [invokeSync, invokeJob, serveStream] {
        #expect(throws: Never.self) {
            _ = try RuntimeManifest.load(table: try TOMLLite.parse(good), directory: nil)
        }
    }
}

@Test func manifestIdSlugValidation() throws {
    func manifest(id: String) -> String {
        "id = \"\(id)\"\ncapabilities = [\"chat\"]\nexecution = \"sync\"\n[invoke]\ncommand = \"x\""
    }
    for bad in ["../evil", "a/b", "..", "…", "a b"] {
        #expect(throws: ManifestValidationError.self) {
            _ = try RuntimeManifest.load(table: try TOMLLite.parse(manifest(id: bad)), directory: nil)
        }
    }
    for good in ["python:kokoro-vm", "my-runtime_2.0", "a.b.c"] {
        #expect(throws: Never.self) {
            _ = try RuntimeManifest.load(table: try TOMLLite.parse(manifest(id: good)), directory: nil)
        }
    }
}

@Test func environmentSanitizerNeutralizesTraversalIDs() {
    #expect(EnvironmentManager.sanitizedRuntimeID("python:kokoro-vm") == "python-kokoro-vm")
    #expect(!EnvironmentManager.sanitizedRuntimeID("x/../../y").contains("/"))
    #expect(!EnvironmentManager.sanitizedRuntimeID("../evil").contains("/"))
    #expect(!EnvironmentManager.sanitizedRuntimeID("a/b").contains("/"))
}

@Test func errorSummaryTakesLastMeaningfulLine() {
    let traceback = """
        Traceback (most recent call last):
          File "main.py", line 12, in <module>
            raise ValueError("bad size")
        ValueError: bad size
        """
    #expect(ManifestSupport.errorSummary(traceback) == "ValueError: bad size")
    #expect(ManifestSupport.errorSummary("   ") == "the runtime stopped without output")
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
