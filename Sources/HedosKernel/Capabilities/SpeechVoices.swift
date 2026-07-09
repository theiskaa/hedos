import Foundation

public enum SpeechVoices {
    static let weightExtensions: Set<String> = ["safetensors", "pt", "bin", "npz"]

    public static func available(_ record: ModelRecord) -> [String] {
        let paths = SidecarModelPaths.resolve(record)
        let directory = URL(fileURLWithPath: paths.snapshot).appendingPathComponent("voices")
        let entries = (try? FileManager.default.contentsOfDirectory(atPath: directory.path)) ?? []
        var names: Set<String> = []
        for entry in entries {
            guard !entry.hasPrefix(".") else { continue }
            let url = URL(fileURLWithPath: entry)
            guard weightExtensions.contains(url.pathExtension.lowercased()) else { continue }
            let name = url.deletingPathExtension().lastPathComponent
            guard !name.isEmpty else { continue }
            names.insert(name)
        }
        return names.sorted()
    }
}
