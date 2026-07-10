import Foundation

enum Footprint {
    static func weightsMB(path: String) -> Int? {
        guard
            let size = try? FileManager.default
                .attributesOfItem(atPath: path)[.size] as? Int64
        else { return nil }
        return Int(size / (1 << 20))
    }

    static func directoryMB(path: String) -> Int? {
        let url = URL(fileURLWithPath: path)
        guard
            let enumerator = FileManager.default.enumerator(
                at: url, includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey])
        else { return nil }
        var total = 0
        for case let entry as URL in enumerator {
            let values = try? entry.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey])
            if values?.isRegularFile == true { total += values?.fileSize ?? 0 }
        }
        return total / (1 << 20)
    }
}
