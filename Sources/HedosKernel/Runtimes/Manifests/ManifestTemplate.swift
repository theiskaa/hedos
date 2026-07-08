import Foundation

public enum ManifestTemplate {
    public static func render(record: ModelRecord, identified: IdentifiedModel) -> String {
        let slug = ManifestSupport.slug(record.displayName.lowercased())
        let modality = identified.modality ?? record.modality
        let capabilities = defaultCapabilities(for: modality)
        let execution = modality == .image ? "job" : "stream"
        let detect = detectLine(record: record, identified: identified)
        return """
            id           = "\(slug)"
            modalities   = ["\(modality.rawValue)"]
            capabilities = [\(capabilities.map { "\"\($0.rawValue)\"" }.joined(separator: ", "))]
            execution    = "\(execution)"
            \(detect)

            [env]
            manager  = "uv"
            python   = "3.12"
            lockfile = "requirements.lock"

            [serve]
            entrypoint = "main.py"
            protocol   = "ndjson+frames"

            # or replace [env]+[serve] with a one-shot command:
            # [invoke]
            # command = "your-tool --model {model} --prompt {prompt} --out {outputs}"

            [permissions]
            network = false
            paths   = ["{model}", "{workdir}"]
            """
    }

    static func defaultCapabilities(for modality: Modality) -> [Capability] {
        switch modality {
        case .image: return [.image]
        case .speech: return [.speak]
        case .audio: return [.transcribe]
        default: return [.chat, .complete]
        }
    }

    static func detectLine(record: ModelRecord, identified: IdentifiedModel) -> String {
        if identified.format == .diffusers, let pipelineClass = identified.pipelineClass {
            return
                "detect       = { file = \"model_index.json\", contains = \"\(pipelineClass)\" }"
        }
        if let weight = record.primaryWeightPath {
            let ext = (weight as NSString).pathExtension.lowercased()
            if !ext.isEmpty {
                return "detect       = { extension = \"\(ext)\" }"
            }
        }
        if let architecture = configArchitecture(record: record) {
            return "detect       = { file = \"config.json\", contains = \"\(architecture)\" }"
        }
        return "detect       = { file = \"config.json\" }"
    }

    static func configArchitecture(record: ModelRecord) -> String? {
        let paths = SidecarModelPaths.resolve(record)
        let config = URL(fileURLWithPath: paths.snapshot).appendingPathComponent("config.json")
        guard let data = try? Data(contentsOf: config),
            let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
            let architectures = json["architectures"] as? [String]
        else { return nil }
        return architectures.first
    }
}
