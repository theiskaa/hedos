import Foundation

public struct DayUsage: Sendable, Hashable {
    public let day: Date
    public let messages: Int
    public let promptTokens: Int
    public let completionTokens: Int

    public var tokens: Int { promptTokens + completionTokens }

    public init(day: Date, messages: Int, promptTokens: Int, completionTokens: Int) {
        self.day = day
        self.messages = messages
        self.promptTokens = promptTokens
        self.completionTokens = completionTokens
    }
}
