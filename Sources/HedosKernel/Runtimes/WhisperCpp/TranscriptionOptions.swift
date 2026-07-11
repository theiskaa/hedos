import Foundation

public struct TranscriptionOptions: Sendable, Hashable {
    public var language: String?
    public var translate: Bool

    public init(language: String? = nil, translate: Bool = false) {
        self.language = language
        self.translate = translate
    }
}

public struct TranscriptionSegment: Sendable, Hashable {
    public var text: String
    public var startMs: Int?
    public var endMs: Int?

    public init(text: String, startMs: Int? = nil, endMs: Int? = nil) {
        self.text = text
        self.startMs = startMs
        self.endMs = endMs
    }
}
