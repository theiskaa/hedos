import Foundation
import Testing

@testable import Hedos

private func entry(
    _ id: UUID, disabled: Bool = false, chevron: Bool = false, initial: Bool = false
) -> KeyNavEntry {
    KeyNavEntry(id: id, disabled: disabled, chevron: chevron, initial: initial)
}

@Test func moveWrapsAtBothEnds() {
    let ids = [UUID(), UUID(), UUID()]
    let entries = ids.map { entry($0) }
    #expect(KeyNavMath.move(from: ids[2], delta: 1, in: entries) == ids[0])
    #expect(KeyNavMath.move(from: ids[0], delta: -1, in: entries) == ids[2])
    #expect(KeyNavMath.move(from: ids[0], delta: 1, in: entries) == ids[1])
}

@Test func moveSkipsDisabledInBothDirections() {
    let ids = [UUID(), UUID(), UUID()]
    let entries = [entry(ids[0]), entry(ids[1], disabled: true), entry(ids[2])]
    #expect(KeyNavMath.move(from: ids[0], delta: 1, in: entries) == ids[2])
    #expect(KeyNavMath.move(from: ids[2], delta: -1, in: entries) == ids[0])
}

@Test func moveWithNoCurrentStartsAtTheNearestEnd() {
    let ids = [UUID(), UUID()]
    let entries = ids.map { entry($0) }
    #expect(KeyNavMath.move(from: nil, delta: 1, in: entries) == ids[0])
    #expect(KeyNavMath.move(from: nil, delta: -1, in: entries) == ids[1])
}

@Test func moveReturnsNilWhenAllDisabled() {
    let entries = [entry(UUID(), disabled: true), entry(UUID(), disabled: true)]
    #expect(KeyNavMath.move(from: nil, delta: 1, in: entries) == nil)
    #expect(KeyNavMath.move(from: nil, delta: -1, in: entries) == nil)
}

@Test func gridColumnsMirrorAdaptiveLayout() {
    #expect(GridKeyNav.columns(width: 900, minItem: 280, spacing: 12) == 3)
    #expect(GridKeyNav.columns(width: 560, minItem: 280, spacing: 12) == 1)
    #expect(GridKeyNav.columns(width: 592, minItem: 280, spacing: 12) == 2)
    #expect(GridKeyNav.columns(width: 0, minItem: 280, spacing: 12) == 1)
}

@Test func gridMoveClampsAtEdges() {
    #expect(GridKeyNav.move(index: 0, direction: .left, columns: 3, count: 7) == 0)
    #expect(GridKeyNav.move(index: 0, direction: .right, columns: 3, count: 7) == 1)
    #expect(GridKeyNav.move(index: 2, direction: .down, columns: 3, count: 7) == 5)
    #expect(GridKeyNav.move(index: 4, direction: .up, columns: 3, count: 7) == 1)
    #expect(GridKeyNav.move(index: 6, direction: .right, columns: 3, count: 7) == 6)
    #expect(GridKeyNav.move(index: 6, direction: .down, columns: 3, count: 7) == 6)
    #expect(GridKeyNav.move(index: 1, direction: .up, columns: 3, count: 7) == 1)
}

@Test func gridMoveDownIntoShorterLastRowClampsToLastItem() {
    #expect(GridKeyNav.move(index: 5, direction: .down, columns: 3, count: 7) == 6)
    #expect(GridKeyNav.move(index: 4, direction: .down, columns: 3, count: 7) == 6)
}

@Test func gridMoveCrossesSectionBoundariesByColumn() {
    let sections = [5, 4]
    #expect(GridKeyNav.move(index: 4, direction: .down, columns: 4, sections: sections) == 5)
    #expect(GridKeyNav.move(index: 1, direction: .down, columns: 4, sections: sections) == 4)
    #expect(GridKeyNav.move(index: 5, direction: .up, columns: 4, sections: sections) == 4)
    #expect(GridKeyNav.move(index: 7, direction: .up, columns: 4, sections: sections) == 4)
    #expect(GridKeyNav.move(index: 0, direction: .up, columns: 4, sections: sections) == 0)
    #expect(GridKeyNav.move(index: 8, direction: .down, columns: 4, sections: sections) == 8)
}

@Test func gridMoveAcrossSectionsKeepsColumnWhenRowsAlign() {
    let sections = [4, 4]
    #expect(GridKeyNav.move(index: 2, direction: .down, columns: 4, sections: sections) == 6)
    #expect(GridKeyNav.move(index: 6, direction: .up, columns: 4, sections: sections) == 2)
    #expect(GridKeyNav.move(index: 3, direction: .right, columns: 4, sections: sections) == 4)
    #expect(GridKeyNav.move(index: 4, direction: .left, columns: 4, sections: sections) == 3)
}

@Test func reconcileKeepsCurrentPrefersInitialThenFirstEnabled() {
    let ids = [UUID(), UUID(), UUID()]
    let entries = [
        entry(ids[0], disabled: true), entry(ids[1], initial: true), entry(ids[2]),
    ]
    #expect(KeyNavMath.reconcile(current: ids[2], entries: entries) == ids[2])
    #expect(KeyNavMath.reconcile(current: nil, entries: entries) == ids[1])
    #expect(KeyNavMath.reconcile(current: ids[0], entries: entries) == ids[1])
    #expect(
        KeyNavMath.reconcile(
            current: nil, entries: [entry(ids[0], disabled: true), entry(ids[2])]) == ids[2])
    #expect(KeyNavMath.reconcile(current: nil, entries: []) == nil)
}
