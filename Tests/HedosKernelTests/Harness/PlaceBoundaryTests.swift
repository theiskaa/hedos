import Foundation
import Testing

@testable import HedosKernel

private func makePlace() throws -> (place: String, dir: URL) {
    let dir = try Fixtures.tempDirectory()
    try FileManager.default.createDirectory(
        at: dir.appendingPathComponent("sub"), withIntermediateDirectories: true)
    try Data("hello".utf8).write(to: dir.appendingPathComponent("file.txt"))
    try Data("nested".utf8).write(to: dir.appendingPathComponent("sub/inner.txt"))
    return (PlaceBoundary.canonical(dir.path), dir)
}

@Test func happyPathsResolveInsideThePlace() throws {
    let (place, dir) = try makePlace()
    defer { try? FileManager.default.removeItem(at: dir) }

    #expect(try PlaceBoundary.resolve(".", in: place) == place)
    #expect(try PlaceBoundary.resolve("", in: place) == place)
    #expect(try PlaceBoundary.resolve("file.txt", in: place) == place + "/file.txt")
    #expect(try PlaceBoundary.resolve("sub/inner.txt", in: place) == place + "/sub/inner.txt")
    #expect(try PlaceBoundary.resolve(place + "/file.txt", in: place) == place + "/file.txt")
}

@Test func dotDotTraversalIsRefused() throws {
    let (place, dir) = try makePlace()
    defer { try? FileManager.default.removeItem(at: dir) }

    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve("../", in: place)
    }
    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve("../../etc/passwd", in: place)
    }
    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve("sub/../../outside.txt", in: place)
    }
}

@Test func absolutePathsOutsideThePlaceAreRefused() throws {
    let (place, dir) = try makePlace()
    defer { try? FileManager.default.removeItem(at: dir) }

    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve("/etc/passwd", in: place)
    }
    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve("~/anything", in: place)
    }
}

@Test func prefixCollisionNamesNeverPass() throws {
    let (place, dir) = try makePlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let evil = place + "-evil"
    try FileManager.default.createDirectory(
        atPath: evil, withIntermediateDirectories: true)
    defer { try? FileManager.default.removeItem(atPath: evil) }
    try Data("secret".utf8).write(to: URL(fileURLWithPath: evil + "/loot.txt"))

    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve(evil + "/loot.txt", in: place)
    }
}

@Test func symlinkInsidePointingOutsideIsRefused() throws {
    let (place, dir) = try makePlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let outside = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: outside) }
    try Data("secret".utf8).write(to: outside.appendingPathComponent("loot.txt"))
    try FileManager.default.createSymbolicLink(
        atPath: place + "/escape", withDestinationPath: outside.path)

    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve("escape/loot.txt", in: place)
    }
    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve("escape", in: place)
    }
}

@Test func symlinkedPlaceItselfResolvesThroughItsCanonicalForm() throws {
    let (place, dir) = try makePlace()
    defer { try? FileManager.default.removeItem(at: dir) }
    let alias = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: alias) }
    let link = alias.appendingPathComponent("link").path
    try FileManager.default.createSymbolicLink(
        atPath: link, withDestinationPath: place)

    let canonicalPlace = PlaceBoundary.canonical(link)
    #expect(canonicalPlace == place)
    #expect(try PlaceBoundary.resolve("file.txt", in: canonicalPlace) == place + "/file.txt")
}

@Test func nonexistentTailsResolveLexicallyWithoutDotComponents() throws {
    let (place, dir) = try makePlace()
    defer { try? FileManager.default.removeItem(at: dir) }

    #expect(
        try PlaceBoundary.resolve("sub/not-yet-here.txt", in: place)
            == place + "/sub/not-yet-here.txt")
    #expect(throws: HarnessError.self) {
        _ = try PlaceBoundary.resolve("missing/../file.txt", in: place)
    }
}
