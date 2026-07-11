import Foundation

public struct PlaceIgnore: Sendable {
    struct Rule: Sendable {
        var negated: Bool
        var directoryOnly: Bool
        var anchored: Bool
        var pattern: String
    }

    private let rules: [Rule]

    public static func load(place: String) -> PlaceIgnore {
        guard let data = FileManager.default.contents(atPath: place + "/.gitignore"),
            let text = String(data: data, encoding: .utf8)
        else {
            return PlaceIgnore(lines: [])
        }
        return PlaceIgnore(lines: text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init))
    }

    public init(lines: [String]) {
        var parsed: [Rule] = []
        for rawLine in lines {
            var line = rawLine
            while line.hasSuffix(" ") && !line.hasSuffix("\\ ") {
                line.removeLast()
            }
            guard !line.isEmpty, !line.hasPrefix("#") else { continue }
            var negated = false
            if line.hasPrefix("!") {
                negated = true
                line.removeFirst()
            }
            var directoryOnly = false
            if line.hasSuffix("/") {
                directoryOnly = true
                line.removeLast()
            }
            var anchored = false
            if line.hasPrefix("/") {
                anchored = true
                line.removeFirst()
            }
            if line.contains("/") {
                anchored = true
            }
            guard !line.isEmpty else { continue }
            parsed.append(
                Rule(
                    negated: negated, directoryOnly: directoryOnly,
                    anchored: anchored, pattern: line))
        }
        rules = parsed
    }

    public var isEmpty: Bool {
        rules.isEmpty
    }

    public func ignored(_ relativePath: String, isDirectory: Bool) -> Bool {
        guard !rules.isEmpty, !relativePath.isEmpty else { return false }
        var ancestor = (relativePath as NSString).deletingLastPathComponent
        while !ancestor.isEmpty {
            if directDecision(ancestor, isDirectory: true) == true { return true }
            ancestor = (ancestor as NSString).deletingLastPathComponent
        }
        return directDecision(relativePath, isDirectory: isDirectory) ?? false
    }

    private func directDecision(_ path: String, isDirectory: Bool) -> Bool? {
        var verdict: Bool?
        for rule in rules {
            if matches(rule, path: path, isDirectory: isDirectory) {
                verdict = !rule.negated
            }
        }
        return verdict
    }

    private func matches(_ rule: Rule, path: String, isDirectory: Bool) -> Bool {
        if rule.directoryOnly && !isDirectory { return false }
        let candidate = rule.anchored ? path : (path as NSString).lastPathComponent
        return Self.wildMatch(rule.pattern, candidate, pathAware: rule.anchored)
    }

    static func wildMatch(_ pattern: String, _ string: String, pathAware: Bool) -> Bool {
        if pattern.contains("**") {
            return regexMatch(pattern, string)
        }
        let flags: Int32 = pathAware ? FNM_PATHNAME : 0
        return fnmatch(pattern, string, flags) == 0
    }

    private static func regexMatch(_ pattern: String, _ string: String) -> Bool {
        var regex = "^"
        var index = pattern.startIndex
        while index < pattern.endIndex {
            let character = pattern[index]
            if character == "*" {
                let next = pattern.index(after: index)
                if next < pattern.endIndex, pattern[next] == "*" {
                    let afterPair = pattern.index(after: next)
                    if afterPair < pattern.endIndex, pattern[afterPair] == "/" {
                        regex += "(?:[^/]*/)*"
                        index = pattern.index(after: afterPair)
                        continue
                    }
                    regex += ".*"
                    index = afterPair
                    continue
                }
                regex += "[^/]*"
                index = next
                continue
            }
            if character == "?" {
                regex += "[^/]"
            } else if "\\^$.|+()[]{}".contains(character) {
                regex += "\\" + String(character)
            } else {
                regex += String(character)
            }
            index = pattern.index(after: index)
        }
        regex += "$"
        return string.range(of: regex, options: .regularExpression) != nil
    }
}
