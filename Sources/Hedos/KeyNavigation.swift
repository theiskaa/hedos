import AppKit
import Carbon.HIToolbox
import SwiftUI

struct KeyNavEntry: Equatable, Identifiable {
    let id: UUID
    let disabled: Bool
    let chevron: Bool
    let initial: Bool
}

struct KeyNavEntriesKey: PreferenceKey {
    static let defaultValue: [KeyNavEntry] = []
    static func reduce(value: inout [KeyNavEntry], nextValue: () -> [KeyNavEntry]) {
        value.append(contentsOf: nextValue())
    }
}

enum KeyNavMath {
    static func move(from current: UUID?, delta: Int, in entries: [KeyNavEntry]) -> UUID? {
        let enabled = entries.filter { !$0.disabled }
        guard !enabled.isEmpty else { return nil }
        guard let current, let index = enabled.firstIndex(where: { $0.id == current }) else {
            return delta >= 0 ? enabled.first?.id : enabled.last?.id
        }
        let count = enabled.count
        return enabled[(index + delta % count + count) % count].id
    }

    static func reconcile(current: UUID?, entries: [KeyNavEntry]) -> UUID? {
        if let current, entries.contains(where: { $0.id == current && !$0.disabled }) {
            return current
        }
        if let preferred = entries.first(where: { $0.initial && !$0.disabled }) {
            return preferred.id
        }
        return entries.first { !$0.disabled }?.id
    }
}

enum GridKeyNav {
    static func columns(width: CGFloat, minItem: CGFloat, spacing: CGFloat) -> Int {
        guard width > 0, minItem > 0 else { return 1 }
        return max(1, Int((width + spacing) / (minItem + spacing)))
    }

    static func move(index: Int, direction: MoveCommandDirection, columns: Int, count: Int)
        -> Int
    {
        move(index: index, direction: direction, columns: columns, sections: [count])
    }

    static func move(
        index: Int, direction: MoveCommandDirection, columns: Int, sections: [Int]
    ) -> Int {
        let counts = sections.filter { $0 > 0 }
        let total = counts.reduce(0, +)
        guard total > 0 else { return 0 }
        let current = max(0, min(index, total - 1))
        let cols = max(1, columns)
        switch direction {
        case .left:
            return max(0, current - 1)
        case .right:
            return min(total - 1, current + 1)
        case .up, .down:
            break
        @unknown default:
            return current
        }
        var start = 0
        var section = 0
        for (position, count) in counts.enumerated() {
            if current < start + count {
                section = position
                break
            }
            start += count
        }
        let local = current - start
        let row = local / cols
        let column = local % cols
        if direction == .down {
            let lastRow = (counts[section] - 1) / cols
            if row < lastRow {
                return start + min(local + cols, counts[section] - 1)
            }
            guard section + 1 < counts.count else { return current }
            return start + counts[section] + min(column, counts[section + 1] - 1)
        }
        if row > 0 {
            return start + local - cols
        }
        guard section > 0 else { return current }
        let previousCount = counts[section - 1]
        let previousStart = start - previousCount
        let lastRowStart = ((previousCount - 1) / cols) * cols
        return previousStart + min(lastRowStart + column, previousCount - 1)
    }
}

struct KeyNavActivation: Equatable {
    let id: UUID
    let tick: Int
}

@Observable
@MainActor
final class KeyNavCoordinator {
    private static var captureStack: [ObjectIdentifier] = []

    static var capturing: Bool { !captureStack.isEmpty }

    static func pushCapture(_ coordinator: KeyNavCoordinator) {
        captureStack.append(ObjectIdentifier(coordinator))
    }

    static func popCapture(_ coordinator: KeyNavCoordinator) {
        captureStack.removeAll { $0 == ObjectIdentifier(coordinator) }
    }

    static func isTopCapture(_ coordinator: KeyNavCoordinator) -> Bool {
        captureStack.last == ObjectIdentifier(coordinator)
    }

    private(set) var entries: [KeyNavEntry] = []
    private(set) var highlightedID: UUID?
    private(set) var activation: KeyNavActivation?
    @ObservationIgnored var escapeOverride: (() -> Bool)?
    @ObservationIgnored var leftOverride: (() -> Bool)?
    @ObservationIgnored private var tick = 0

    func setEntries(_ entries: [KeyNavEntry]) {
        self.entries = entries
        highlightedID = KeyNavMath.reconcile(current: highlightedID, entries: entries)
    }

    func move(_ delta: Int) {
        highlightedID = KeyNavMath.move(from: highlightedID, delta: delta, in: entries)
    }

    func highlight(_ id: UUID) {
        guard entries.contains(where: { $0.id == id && !$0.disabled }) else { return }
        highlightedID = id
    }

    func activateHighlighted() {
        guard let highlightedID else { return }
        tick += 1
        activation = KeyNavActivation(id: highlightedID, tick: tick)
    }

    func activateChevron() -> Bool {
        guard let highlightedID,
            entries.contains(where: { $0.id == highlightedID && $0.chevron })
        else { return false }
        activateHighlighted()
        return true
    }
}

private struct KeyNavKey: EnvironmentKey {
    static let defaultValue: KeyNavCoordinator? = nil
}

extension EnvironmentValues {
    var keyNav: KeyNavCoordinator? {
        get { self[KeyNavKey.self] }
        set { self[KeyNavKey.self] = newValue }
    }
}

private struct KeyNavRow: ViewModifier {
    let id: UUID
    let disabled: Bool
    let chevron: Bool
    let initial: Bool
    let trigger: () -> Void
    @Environment(\.keyNav) private var keyNav

    func body(content: Content) -> some View {
        content
            .preference(
                key: KeyNavEntriesKey.self,
                value: [KeyNavEntry(id: id, disabled: disabled, chevron: chevron, initial: initial)]
            )
            .id(id)
            .onChange(of: keyNav?.activation) { _, request in
                if request?.id == id, !disabled {
                    trigger()
                }
            }
    }
}

extension View {
    func keyNavRow(
        id: UUID, disabled: Bool, chevron: Bool = false, initial: Bool = false,
        trigger: @escaping () -> Void
    ) -> some View {
        modifier(
            KeyNavRow(id: id, disabled: disabled, chevron: chevron, initial: initial, trigger: trigger))
    }
}

private struct KeyNavContainer: ViewModifier {
    let coordinator: KeyNavCoordinator
    let capturesKeys: Bool
    let onDismiss: (@MainActor () -> Void)?
    @State private var monitor: Any?

    func body(content: Content) -> some View {
        content
            .environment(\.keyNav, coordinator)
            .onPreferenceChange(KeyNavEntriesKey.self) { entries in
                MainActor.assumeIsolated {
                    coordinator.setEntries(entries)
                }
            }
            .onAppear { installMonitorIfNeeded() }
            .onDisappear { removeMonitor() }
    }

    private func installMonitorIfNeeded() {
        guard capturesKeys, monitor == nil else { return }
        KeyNavCoordinator.pushCapture(coordinator)
        monitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            handle(event)
        }
    }

    private func removeMonitor() {
        if let monitor {
            NSEvent.removeMonitor(monitor)
        }
        monitor = nil
        KeyNavCoordinator.popCapture(coordinator)
    }

    private func handle(_ event: NSEvent) -> NSEvent? {
        guard KeyNavCoordinator.isTopCapture(coordinator) else { return event }
        guard
            event.modifierFlags.intersection([.command, .option, .control, .shift]).isEmpty
        else {
            return event
        }
        switch keyIntent(for: event) {
        case .up:
            guard !coordinator.entries.isEmpty else { return event }
            coordinator.move(-1)
            return nil
        case .down:
            guard !coordinator.entries.isEmpty else { return event }
            coordinator.move(1)
            return nil
        case .left:
            if coordinator.leftOverride?() == true { return nil }
            return coordinator.entries.isEmpty ? event : nil
        case .right:
            if coordinator.activateChevron() { return nil }
            return coordinator.entries.isEmpty ? event : nil
        case .commit:
            guard coordinator.highlightedID != nil else { return event }
            coordinator.activateHighlighted()
            return nil
        case .dismiss:
            if coordinator.escapeOverride?() == true {
                return nil
            }
            if let onDismiss {
                onDismiss()
                return nil
            }
            return event
        case nil:
            return event
        }
    }

    private enum KeyIntent {
        case up, down, left, right, commit, dismiss
    }

    private func keyIntent(for event: NSEvent) -> KeyIntent? {
        switch Int(event.keyCode) {
        case kVK_UpArrow: return .up
        case kVK_DownArrow: return .down
        case kVK_LeftArrow: return .left
        case kVK_RightArrow: return .right
        case kVK_Return, kVK_ANSI_KeypadEnter: return .commit
        case kVK_Escape: return .dismiss
        default:
            switch vimDirection(event.charactersIgnoringModifiers) {
            case .up: return .up
            case .down: return .down
            case .left: return .left
            case .right: return .right
            default: return nil
            }
        }
    }
}

func vimDirection(_ characters: String?) -> MoveCommandDirection? {
    switch characters?.lowercased() {
    case "k": return .up
    case "j": return .down
    case "h": return .left
    case "l": return .right
    default: return nil
    }
}

extension View {
    func vimMoveCommand(
        when active: Bool = true, _ perform: @escaping (MoveCommandDirection) -> Void
    ) -> some View {
        onKeyPress(characters: CharacterSet(charactersIn: "hjklHJKL"), phases: .down) { press in
            guard active, press.modifiers.subtracting(.capsLock).isEmpty,
                let direction = vimDirection(press.characters)
            else { return .ignored }
            perform(direction)
            return .handled
        }
    }

    func keyedGridRing(_ on: Bool) -> some View {
        overlay(
            RoundedRectangle.soft(Design.Radius.tile)
                .strokeBorder(
                    on ? Design.accent.opacity(0.55) : .clear,
                    lineWidth: Design.hairlineWidth))
    }

    func keyNavigableList(
        coordinator: KeyNavCoordinator, capturesKeys: Bool = true,
        onDismiss: (@MainActor () -> Void)? = nil
    ) -> some View {
        modifier(
            KeyNavContainer(
                coordinator: coordinator, capturesKeys: capturesKeys, onDismiss: onDismiss))
    }
}
