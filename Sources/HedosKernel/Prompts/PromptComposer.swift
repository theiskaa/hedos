import Foundation

public enum PromptComposer {
    public static func tokenRange(in draft: String) -> Range<String.Index>? {
        guard let slash = draft.lastIndex(of: "/") else { return nil }
        if slash > draft.startIndex {
            let before = draft[draft.index(before: slash)]
            guard before.isWhitespace else { return nil }
        }
        let token = draft[draft.index(after: slash)...]
        guard !token.contains(where: \.isWhitespace) else { return nil }
        return slash..<draft.endIndex
    }

    public static func query(in draft: String) -> String? {
        guard let range = tokenRange(in: draft) else { return nil }
        return String(draft[range].dropFirst())
    }

    public static func matchScore(_ query: String, against title: String) -> Int? {
        guard !query.isEmpty else { return 3 }
        let needle = query.lowercased()
        let haystack = title.lowercased()
        if haystack.hasPrefix(needle) { return 0 }
        if haystack.contains(needle) { return 1 }
        var position = needle.startIndex
        for character in haystack where position < needle.endIndex {
            if character == needle[position] {
                position = needle.index(after: position)
            }
        }
        return position == needle.endIndex ? 2 : nil
    }

    public static func inserting(_ prompt: Prompt, into draft: String) -> String {
        guard let range = tokenRange(in: draft) else { return draft }
        let prefix = String(draft[..<range.lowerBound])
        let selection = prefix.trimmingCharacters(in: .whitespacesAndNewlines)
        let resolved = prompt.resolvedBody(["selection": selection])
        let consumed = prompt.placeholderNames.contains("selection") && !selection.isEmpty
        return consumed ? resolved : prefix + resolved
    }

    public static func clearingToken(from draft: String) -> String {
        guard let range = tokenRange(in: draft) else { return draft }
        return String(draft[..<range.lowerBound])
    }
}
