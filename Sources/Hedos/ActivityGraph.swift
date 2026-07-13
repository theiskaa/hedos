import HedosKernel
import SwiftUI

struct ActivityGraph: View {
    let usage: [DayUsage]
    let loaded: Bool

    @State private var hovered: Date? = nil
    @State private var appeared = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private static let weeks = 53
    private static let maxCell: CGFloat = 10
    private static let minCell: CGFloat = 7
    private static let gap: CGFloat = 2
    private static let leftInset: CGFloat = 22
    private static let topInset: CGFloat = 14

    private let today: Date
    private let gridStart: Date
    private let visible: [DayUsage]
    private let byDay: [Date: DayUsage]
    private let thresholds: [Int]

    init(usage: [DayUsage], loaded: Bool) {
        self.usage = usage
        self.loaded = loaded

        let calendar = Calendar.current
        let today = calendar.startOfDay(for: .now)
        let weekday = calendar.component(.weekday, from: today)
        let offset = (weekday - calendar.firstWeekday + 7) % 7
        let weekStart = calendar.date(byAdding: .day, value: -offset, to: today) ?? today
        let start = calendar.date(byAdding: .day, value: -(Self.weeks - 1) * 7, to: weekStart)
            ?? today

        self.today = today
        self.gridStart = start
        let visible = usage.filter { $0.day >= start }
        self.visible = visible
        self.byDay = Dictionary(visible.map { ($0.day, $0) }, uniquingKeysWith: { first, _ in first })

        let counts = visible.map(\.messages).filter { $0 > 0 }.sorted()
        if counts.isEmpty {
            self.thresholds = [1, 1, 1, 1]
        } else {
            func quantile(_ p: Double) -> Int {
                counts[min(counts.count - 1, Int(Double(counts.count - 1) * p))]
            }
            self.thresholds = [quantile(0.25), quantile(0.5), quantile(0.75), quantile(1)]
        }
    }

    private var calendar: Calendar { Calendar.current }

    private var gridHeight: CGFloat {
        Self.topInset + 7 * Self.maxCell + 6 * Self.gap
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Design.Space.m) {
            if !usage.isEmpty {
                header
                grid
                    .frame(height: gridHeight)
                    .opacity(appeared || reduceMotion ? 1 : 0)
                    .onAppear {
                        withAnimation(Design.motion(reduceMotion: reduceMotion)) { appeared = true }
                    }
            } else if loaded {
                Text("No chats yet — your activity shows up here as you use the app.")
                    .font(Design.caption)
                    .foregroundStyle(Design.inkSoft)
                    .padding(.vertical, Design.Space.s)
            } else {
                SkeletonPulse(radius: Design.Radius.control)
                    .frame(height: gridHeight)
            }
        }
    }

    private var grid: some View {
        GeometryReader { geometry in
            let cell = cellSize(for: geometry.size.width)
            Canvas { context, size in
                draw(in: context, size: size, cell: cell)
            }
            .onContinuousHover { phase in
                let next: Date?
                switch phase {
                case .active(let point): next = day(at: point, cell: cell)
                case .ended: next = nil
                }
                if next != hovered {
                    withAnimation(Design.wash) { hovered = next }
                }
            }
        }
    }

    private func cellSize(for width: CGFloat) -> CGFloat {
        let usable = width - Self.leftInset - CGFloat(Self.weeks - 1) * Self.gap
        return min(Self.maxCell, max(Self.minCell, usable / CGFloat(Self.weeks)))
    }

    private func origin(column: Int, row: Int, cell: CGFloat) -> CGPoint {
        CGPoint(
            x: Self.leftInset + CGFloat(column) * (cell + Self.gap),
            y: Self.topInset + CGFloat(row) * (cell + Self.gap))
    }

    private func dayAt(column: Int, row: Int) -> Date {
        calendar.date(byAdding: .day, value: column * 7 + row, to: gridStart) ?? gridStart
    }

    private func draw(in context: GraphicsContext, size: CGSize, cell: CGFloat) {
        let step = cell + Self.gap
        for column in 0..<Self.weeks {
            if let label = monthLabel(column) {
                context.draw(
                    Text(label).font(Design.label).foregroundStyle(Design.inkFaint),
                    at: CGPoint(x: Self.leftInset + CGFloat(column) * step, y: Self.topInset - 4),
                    anchor: .bottomLeading)
            }
        }
        for row in 0..<7 where row % 2 == 1 {
            context.draw(
                Text(weekdaySymbol(row)).font(Design.label).foregroundStyle(Design.inkFaint),
                at: CGPoint(x: 0, y: Self.topInset + CGFloat(row) * step + cell / 2),
                anchor: .leading)
        }
        for column in 0..<Self.weeks {
            for row in 0..<7 {
                let day = dayAt(column: column, row: row)
                guard day <= today else { continue }
                let point = origin(column: column, row: row, cell: cell)
                let rect = CGRect(x: point.x, y: point.y, width: cell, height: cell)
                let path = Path(roundedRect: rect, cornerRadius: 2, style: .continuous)
                context.fill(path, with: .color(color(level(byDay[day]?.messages ?? 0))))
                if day == hovered {
                    let ring = Path(
                        roundedRect: rect.insetBy(dx: -1, dy: -1), cornerRadius: 3,
                        style: .continuous)
                    context.stroke(ring, with: .color(Design.ink.opacity(0.7)), lineWidth: 1.5)
                }
            }
        }
    }

    private func day(at point: CGPoint, cell: CGFloat) -> Date? {
        let step = cell + Self.gap
        guard point.x >= Self.leftInset, point.y >= Self.topInset else { return nil }
        let column = Int((point.x - Self.leftInset) / step)
        let row = Int((point.y - Self.topInset) / step)
        guard column >= 0, column < Self.weeks, row >= 0, row < 7 else { return nil }
        let day = dayAt(column: column, row: row)
        return day <= today ? day : nil
    }

    private func level(_ messages: Int) -> Int {
        guard messages > 0 else { return 0 }
        if messages >= thresholds[2] { return 4 }
        if messages >= thresholds[1] { return 3 }
        if messages >= thresholds[0] { return 2 }
        return 1
    }

    private func color(_ level: Int) -> Color {
        switch level {
        case 1: Design.accent.opacity(0.22)
        case 2: Design.accent.opacity(0.42)
        case 3: Design.accent.opacity(0.66)
        case 4: Design.accent.opacity(0.92)
        default: Design.inkWash
        }
    }

    private func weekdaySymbol(_ row: Int) -> String {
        let index = (calendar.firstWeekday - 1 + row) % 7
        return String(calendar.shortWeekdaySymbols[index].prefix(1))
    }

    private func monthLabel(_ column: Int) -> String? {
        let day = dayAt(column: column, row: 0)
        let month = calendar.component(.month, from: day)
        guard column > 0 else { return calendar.shortMonthSymbols[month - 1] }
        let previous = dayAt(column: column - 1, row: 0)
        guard month != calendar.component(.month, from: previous) else { return nil }
        return calendar.shortMonthSymbols[month - 1]
    }

    private var header: some View {
        ZStack(alignment: .leading) {
            summaryText.opacity(hovered == nil ? 1 : 0)
            detailText(hovered ?? today).opacity(hovered == nil ? 0 : 1)
        }
        .font(Design.caption)
        .monospacedDigit()
        .lineLimit(1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(Design.wash, value: hovered)
    }

    private func pair(_ value: String, _ label: String) -> Text {
        Text(value).fontWeight(.semibold).foregroundStyle(Design.ink)
            + Text(" \(label)").foregroundStyle(Design.inkSoft)
    }

    private var separator: Text {
        Text("   ·   ").foregroundStyle(Design.line)
    }

    private var summaryText: Text {
        let tokens = visible.reduce(0) { $0 + $1.tokens }
        let messages = visible.reduce(0) { $0 + $1.messages }
        let activeDays = visible.filter { $0.messages > 0 }.count
        return pair(compact(tokens), "tokens") + separator
            + pair(compact(messages), messages == 1 ? "message" : "messages") + separator
            + pair(activeDays.formatted(), "active days") + separator
            + pair(streak().formatted(), "day streak")
    }

    private func detailText(_ day: Date) -> Text {
        let entry = byDay[day]
        let messages = entry?.messages ?? 0
        let tokens = entry?.tokens ?? 0
        return pair(compact(messages), messages == 1 ? "message" : "messages") + separator
            + pair(compact(tokens), "tokens") + separator
            + Text(day.formatted(.dateTime.weekday(.abbreviated).month().day()))
                .foregroundStyle(Design.inkFaint)
    }

    private func compact(_ value: Int) -> String {
        value.formatted(.number.notation(.compactName))
    }

    private func streak() -> Int {
        let active = Set(byDay.filter { $0.value.messages > 0 }.keys)
        var day = active.contains(today)
            ? today : (calendar.date(byAdding: .day, value: -1, to: today) ?? today)
        var count = 0
        while active.contains(day) {
            count += 1
            day = calendar.date(byAdding: .day, value: -1, to: day) ?? day
        }
        return count
    }
}
