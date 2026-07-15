import Foundation

enum HFFileSelection {
    static let quantPreference = ["q4_k_m", "q4_0", "q5_k_m", "q6_k", "q8_0", "f16"]
    static let companionCap: Int64 = 10 << 20
    static let configCap: Int64 = 100 << 20
    static let weightExtensions: Set<String> = [
        "safetensors", "gguf", "bin", "ckpt", "pt", "pth",
    ]
    static let excludedExtensions: Set<String> = [
        "md", "png", "jpg", "jpeg", "gif", "webp", "msgpack", "h5",
    ]
    static let excludedDirectories: Set<String> = ["onnx", "openvino", "coreml"]

    static func select(siblings: [HFSibling]) -> [HFSibling] {
        let kept = siblings.filter(isEligible)
        let ggufs = kept.filter { $0.rfilename.lowercased().hasSuffix(".gguf") }
        if !ggufs.isEmpty {
            return ggufSelection(ggufs: ggufs, others: kept.filter { !ggufs.contains($0) })
        }
        if kept.contains(where: { $0.rfilename == "model_index.json" }) {
            return diffusersSelection(kept)
        }
        return transformersSelection(kept)
    }

    static func isWeight(_ sibling: HFSibling) -> Bool {
        weightExtensions.contains(fileExtension(sibling.rfilename))
    }

    private static func isEligible(_ sibling: HFSibling) -> Bool {
        let path = sibling.rfilename
        let segments = path.split(separator: "/").map(String.init)
        guard let filename = segments.last else { return false }
        if segments.contains(where: { $0.hasPrefix(".") }) { return false }
        if filename.lowercased().hasPrefix("readme") { return false }
        if excludedExtensions.contains(fileExtension(filename)) { return false }
        let stem = filename.lowercased()
        if stem.hasPrefix("flax_model") || stem.hasPrefix("tf_model") { return false }
        if let first = segments.first, segments.count > 1,
            excludedDirectories.contains(first.lowercased())
        {
            return false
        }
        return true
    }

    private static func ggufSelection(ggufs: [HFSibling], others: [HFSibling]) -> [HFSibling] {
        struct GroupKey: Hashable {
            let directory: String
            let base: String
        }
        var groups: [GroupKey: [HFSibling]] = [:]
        for sibling in ggufs {
            let filename = String(sibling.rfilename.split(separator: "/").last ?? "")
            let directory = sibling.rfilename.split(separator: "/").dropLast()
                .joined(separator: "/")
            if let shard = GGUFShards.parse(filename) {
                groups[GroupKey(directory: directory, base: shard.base), default: []]
                    .append(sibling)
            } else {
                let stem = String(filename.dropLast(".gguf".count))
                groups[GroupKey(directory: directory, base: stem), default: []].append(sibling)
            }
        }
        let mmproj = ggufs.filter { $0.rfilename.lowercased().contains("mmproj") }
        let candidates = groups.filter { key, _ in
            !key.base.lowercased().contains("mmproj")
        }
        let chosen = pickQuantGroup(Array(candidates.values))
        let companions = others.filter { ($0.bytes ?? 0) <= companionCap }
        return chosen + mmproj.filter { !chosen.contains($0) } + companions
    }

    private static func pickQuantGroup(_ groups: [[HFSibling]]) -> [HFSibling] {
        guard !groups.isEmpty else { return [] }
        for token in quantPreference {
            if let match = groups.first(where: { group in
                group.contains { $0.rfilename.lowercased().contains(token) }
            }) {
                return match
            }
        }
        let smallest = groups.min { first, second in
            first.compactMap(\.bytes).reduce(0, +) < second.compactMap(\.bytes).reduce(0, +)
        }
        return smallest ?? []
    }

    private static func diffusersSelection(_ kept: [HFSibling]) -> [HFSibling] {
        let paths = Set(kept.map(\.rfilename))
        return kept.filter { sibling in
            let path = sibling.rfilename
            let ext = fileExtension(path)
            if ["bin", "ckpt", "pt", "pth"].contains(ext) {
                let stem = String(path.dropLast(ext.count + 1))
                if paths.contains("\(stem).safetensors") { return false }
            }
            for variant in [".fp16.", ".non_ema."] {
                if path.contains(variant),
                    paths.contains(path.replacingOccurrences(of: variant, with: "."))
                {
                    return false
                }
            }
            return true
        }
    }

    private static func transformersSelection(_ kept: [HFSibling]) -> [HFSibling] {
        let root = kept.filter { !$0.rfilename.contains("/") }
        let safetensors = root.filter {
            fileExtension($0.rfilename) == "safetensors"
                || $0.rfilename.hasSuffix(".safetensors.index.json")
        }
        let weights: [HFSibling]
        if safetensors.contains(where: { fileExtension($0.rfilename) == "safetensors" }) {
            weights = safetensors
        } else {
            weights = root.filter {
                $0.rfilename.hasPrefix("pytorch_model") && (fileExtension($0.rfilename) == "bin"
                    || $0.rfilename.hasSuffix(".bin.index.json"))
            }
        }
        let support = root.filter { sibling in
            !isWeight(sibling) && !sibling.rfilename.hasSuffix(".index.json")
                && (sibling.bytes ?? 0) <= configCap
        }
        return weights + support
    }

    private static func fileExtension(_ path: String) -> String {
        guard let dot = path.lastIndex(of: "."), dot != path.startIndex else { return "" }
        return String(path[path.index(after: dot)...]).lowercased()
    }
}
