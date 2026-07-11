import Foundation
import Testing

@testable import HedosKernel

private func flutterIgnore() -> PlaceIgnore {
    PlaceIgnore(lines: [
        "# Miscellaneous",
        "*.log",
        "",
        "build/",
        ".dart_tool/",
        "/coverage",
        "ios/**/DerivedData/",
        "*.iml",
        "!keep.iml",
        "doc/api/",
    ])
}

@Test func slashlessPatternsMatchBasenamesAtAnyDepth() {
    let ignore = flutterIgnore()
    #expect(ignore.ignored("debug.log", isDirectory: false))
    #expect(ignore.ignored("lib/src/deep/debug.log", isDirectory: false))
    #expect(ignore.ignored("project.iml", isDirectory: false))
    #expect(!ignore.ignored("lib/main.dart", isDirectory: false))
}

@Test func directoryOnlyPatternsExcludeTheTreeBeneath() {
    let ignore = flutterIgnore()
    #expect(ignore.ignored("build", isDirectory: true))
    #expect(ignore.ignored("build/app/outputs/apk/app.apk", isDirectory: false))
    #expect(ignore.ignored("doc/api/index.html", isDirectory: false))
    #expect(!ignore.ignored("build.gradle", isDirectory: false))
    #expect(!ignore.ignored("doc/manual.md", isDirectory: false))
}

@Test func anchoredPatternsOnlyMatchAtTheRoot() {
    let ignore = flutterIgnore()
    #expect(ignore.ignored("coverage", isDirectory: true))
    #expect(ignore.ignored("coverage", isDirectory: false))
    #expect(!ignore.ignored("packages/coverage", isDirectory: true))
}

@Test func doubleStarCrossesDirectories() {
    let ignore = flutterIgnore()
    #expect(ignore.ignored("ios/Pods/DerivedData", isDirectory: true))
    #expect(ignore.ignored("ios/a/b/c/DerivedData/x.txt", isDirectory: false))
    #expect(!ignore.ignored("android/DerivedData", isDirectory: true))
}

@Test func negationWinsWhenItComesLast() {
    let ignore = flutterIgnore()
    #expect(!ignore.ignored("keep.iml", isDirectory: false))
    #expect(ignore.ignored("other.iml", isDirectory: false))

    let reversed = PlaceIgnore(lines: ["!keep.iml", "*.iml"])
    #expect(reversed.ignored("keep.iml", isDirectory: false))
}

@Test func negationCannotReincludeInsideAnExcludedDirectory() {
    let ignore = PlaceIgnore(lines: ["build/", "!build/keep.txt"])
    #expect(ignore.ignored("build/keep.txt", isDirectory: false))
}

@Test func missingGitignoreIgnoresNothing() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let ignore = PlaceIgnore.load(place: dir.path)
    #expect(ignore.isEmpty)
    #expect(!ignore.ignored("build/x", isDirectory: false))
}
