import Foundation
import HedosKernel

enum Capabilities {
    static let known: Set<String> = [
        "chat", "complete", "embed", "see", "image", "speak", "transcribe",
    ]
}

enum Session {
    static func kernel() -> Kernel { Kernel() }

    static func shelf(_ kernel: Kernel, discoverIfEmpty: Bool = true) async throws -> [ModelRecord] {
        var shelf = try await kernel.shelf()
        if shelf.isEmpty && discoverIfEmpty {
            _ = try await kernel.discover()
            shelf = try await kernel.shelf()
        }
        return shelf
    }

    static func resolve(
        _ query: String, in shelf: [ModelRecord], capability: Capability? = nil
    ) throws -> ModelRecord {
        let pool = capability.map { cap in shelf.filter { $0.capabilities.contains(cap) } } ?? shelf
        if let hit = pool.first(where: { $0.id == query }) { return hit }
        if let hit = pool.first(where: {
            $0.name.caseInsensitiveCompare(query) == .orderedSame
                || $0.displayName.caseInsensitiveCompare(query) == .orderedSame
        }) { return hit }
        let matches = pool.filter {
            $0.name.localizedCaseInsensitiveContains(query)
                || $0.displayName.localizedCaseInsensitiveContains(query)
                || $0.id.localizedCaseInsensitiveContains(query)
        }
        if matches.count == 1 { return matches[0] }
        if matches.count > 1 {
            let list = matches.prefix(8)
                .map { "  \($0.id)  ·  \($0.displayName)" }
                .joined(separator: "\n")
            throw CLIError("\"\(query)\" matches \(matches.count) models — be more specific:\n\(list)")
        }
        let hint = capability.map { " serving \($0.rawValue)" } ?? ""
        throw CLIError("no model\(hint) matched \"\(query)\" — run `hedos ls` to see the shelf.")
    }
}
