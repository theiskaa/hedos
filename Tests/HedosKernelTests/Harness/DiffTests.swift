import Foundation
import Testing

@testable import HedosKernel

@Test func diffReportsNoChange() {
    #expect(Diff.unified(from: "a\nb\n", to: "a\nb\n", path: "x").contains("no change"))
}

@Test func diffShowsACreation() {
    let out = Diff.unified(from: "", to: "line one\nline two\n", path: "new.txt")
    #expect(out.contains("+line one"))
    #expect(out.contains("+line two"))
    #expect(!out.contains("-line one"))
}

@Test func diffShowsADeletion() {
    let out = Diff.unified(from: "keep\ngone\n", to: "keep\n", path: "x")
    #expect(out.contains(" keep"))
    #expect(out.contains("-gone"))
    #expect(!out.contains("+gone"))
}

@Test func diffShowsAReplacement() {
    let out = Diff.unified(from: "one\ntwo\nthree\n", to: "one\nTWO\nthree\n", path: "x")
    #expect(out.contains("-two"))
    #expect(out.contains("+TWO"))
    #expect(out.contains(" one"))
    #expect(out.contains(" three"))
}

@Test func diffHandlesEmptyToNonEmptyAndBack() {
    #expect(Diff.unified(from: "x\n", to: "", path: "p").contains("-x"))
    #expect(Diff.unified(from: "", to: "y\n", path: "p").contains("+y"))
}

@Test func diffSurfacesATrailingNewlineOnlyChange() {
    #expect(Diff.unified(from: "a", to: "a\n", path: "x").contains("final newline added"))
    #expect(Diff.unified(from: "a\n", to: "a", path: "x").contains("final newline removed"))
}

@Test func splitLinesDropsTrailingNewlineOnce() {
    #expect(Diff.splitLines("a\nb\n") == ["a", "b"])
    #expect(Diff.splitLines("a\nb") == ["a", "b"])
    #expect(Diff.splitLines("") == [])
}
