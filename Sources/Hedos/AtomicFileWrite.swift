import Foundation

enum AtomicFileWrite {
    static func copy(from source: URL, to destination: URL) throws {
        let temp = destination.deletingLastPathComponent()
            .appendingPathComponent(".hedos-export-\(UUID().uuidString)")
        try? FileManager.default.removeItem(at: temp)
        try FileManager.default.copyItem(at: source, to: temp)
        do {
            if FileManager.default.fileExists(atPath: destination.path) {
                _ = try FileManager.default.replaceItemAt(destination, withItemAt: temp)
            } else {
                try FileManager.default.moveItem(at: temp, to: destination)
            }
        } catch {
            try? FileManager.default.removeItem(at: temp)
            throw error
        }
    }
}
