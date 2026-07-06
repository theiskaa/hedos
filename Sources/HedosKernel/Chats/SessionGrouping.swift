import Foundation

public struct SessionGroup: Sendable, Hashable {
    public let title: String
    public let sessions: [ChatSession]

    public init(title: String, sessions: [ChatSession]) {
        self.title = title
        self.sessions = sessions
    }
}

public enum SessionGrouping {
    public static let pinned = "Pinned"
    public static let today = "Today"
    public static let yesterday = "Yesterday"
    public static let thisWeek = "This Week"
    public static let older = "Older"

    public static func groups(
        _ sessions: [ChatSession], now: Date = Date(), calendar: Calendar = .current
    ) -> [SessionGroup] {
        var buckets: [String: [ChatSession]] = [:]
        for session in sessions {
            buckets[bucket(for: session, now: now, calendar: calendar), default: []]
                .append(session)
        }
        return [pinned, today, yesterday, thisWeek, older].compactMap { title in
            buckets[title].map { SessionGroup(title: title, sessions: $0) }
        }
    }

    private static func bucket(for session: ChatSession, now: Date, calendar: Calendar) -> String {
        if session.pinned { return pinned }
        if calendar.isDate(session.updatedAt, inSameDayAs: now) { return today }
        if let dayBefore = calendar.date(byAdding: .day, value: -1, to: now),
            calendar.isDate(session.updatedAt, inSameDayAs: dayBefore)
        {
            return yesterday
        }
        if let weekAgo = calendar.date(byAdding: .day, value: -7, to: now),
            session.updatedAt > weekAgo
        {
            return thisWeek
        }
        return older
    }
}
