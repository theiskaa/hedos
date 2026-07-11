import Foundation

extension Identification {
    static func safetensorsHeaderMetadataFormat(at url: URL) -> String? {
        guard let handle = FileHandle(forReadingAtPath: url.path),
            let lengthData = try? handle.read(upToCount: 8), lengthData.count == 8
        else { return nil }
        defer { try? handle.close() }
        let length = lengthData.withUnsafeBytes { $0.load(as: UInt64.self) }
        guard length > 0, length < 100_000_000,
            let headerData = try? handle.read(upToCount: Int(length)),
            let header = try? JSONSerialization.jsonObject(with: headerData) as? [String: Any],
            let metadata = header["__metadata__"] as? [String: Any]
        else { return nil }
        return metadata["format"] as? String
    }

    static func safetensorsFormat(in container: URL, configURL: URL) -> ModelFormat? {
        let files =
            (try? FileManager.default.contentsOfDirectory(
                at: container, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles]))
            ?? []
        guard let weight = files.first(where: { $0.pathExtension == "safetensors" }) else {
            return nil
        }

        if let config = try? JSONSerialization.jsonObject(
            with: Data(contentsOf: configURL)) as? [String: Any],
            config["quantization"] != nil
        {
            return .mlxSafetensors
        }
        if safetensorsHeaderMetadataFormat(at: weight.resolvingSymlinksInPath()) == "mlx" {
            return .mlxSafetensors
        }
        return .safetensors
    }
}
