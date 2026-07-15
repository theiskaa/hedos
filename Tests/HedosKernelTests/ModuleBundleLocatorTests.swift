import Foundation
import Testing

@testable import HedosKernel

private func makeRoot() throws -> URL {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("module-bundle-\(UUID().uuidString)")
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    return root
}

@Test func locatorFindsBundleInFirstRoot() throws {
    let root = try makeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let bundleDir = root.appendingPathComponent("hedos_Fake.bundle")
    try FileManager.default.createDirectory(at: bundleDir, withIntermediateDirectories: true)
    let bundle = ModuleBundleLocator.locate(named: "hedos_Fake", roots: [root])
    #expect(
        bundle?.bundleURL.resolvingSymlinksInPath() == bundleDir.resolvingSymlinksInPath())
}

@Test func locatorFallsThroughToLaterRoot() throws {
    let empty = try makeRoot()
    let populated = try makeRoot()
    defer {
        try? FileManager.default.removeItem(at: empty)
        try? FileManager.default.removeItem(at: populated)
    }
    let bundleDir = populated.appendingPathComponent("hedos_Fake.bundle")
    try FileManager.default.createDirectory(at: bundleDir, withIntermediateDirectories: true)
    let bundle = ModuleBundleLocator.locate(named: "hedos_Fake", roots: [empty, populated])
    #expect(
        bundle?.bundleURL.resolvingSymlinksInPath() == bundleDir.resolvingSymlinksInPath())
}

@Test func locatorReturnsNilWhenBundleAbsentEverywhere() throws {
    let first = try makeRoot()
    let second = try makeRoot()
    defer {
        try? FileManager.default.removeItem(at: first)
        try? FileManager.default.removeItem(at: second)
    }
    #expect(ModuleBundleLocator.locate(named: "hedos_Fake", roots: [first, second]) == nil)
}

@Test func kernelModuleBundleResolvesResources() {
    #expect(Bundle.kernelModule.resourceURL != nil)
    #expect(!CommunityLibrary.bundledDirectories().isEmpty)
}

private func makeAppLayout(in root: URL) throws -> (app: URL, resources: URL, helpers: URL) {
    let app = root.appendingPathComponent("Hedos.app")
    let resources = app.appendingPathComponent("Contents/Resources")
    let helpers = app.appendingPathComponent("Contents/Helpers")
    try FileManager.default.createDirectory(
        at: resources.appendingPathComponent("hedos_Fake.bundle"),
        withIntermediateDirectories: true)
    try FileManager.default.createDirectory(at: helpers, withIntermediateDirectories: true)
    try FileManager.default.createDirectory(
        at: app.appendingPathComponent("Contents/MacOS"), withIntermediateDirectories: true)
    return (app, resources, helpers)
}

@Test func defaultRootsCoverThePackagedApp() throws {
    let root = try makeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let layout = try makeAppLayout(in: root)
    let roots = ModuleBundleLocator.defaultRoots(
        resourceURL: layout.resources,
        bundleURL: layout.app,
        executableURL: layout.app.appendingPathComponent("Contents/MacOS/Hedos"))
    #expect(ModuleBundleLocator.locate(named: "hedos_Fake", roots: roots) != nil)
}

@Test func defaultRootsCoverTheHelpersCLI() throws {
    let root = try makeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let layout = try makeAppLayout(in: root)
    let roots = ModuleBundleLocator.defaultRoots(
        resourceURL: layout.helpers,
        bundleURL: layout.helpers,
        executableURL: layout.helpers.appendingPathComponent("hedos"))
    #expect(ModuleBundleLocator.locate(named: "hedos_Fake", roots: roots) != nil)
}

@Test func defaultRootsCoverASymlinkedCLI() throws {
    let root = try makeRoot()
    defer { try? FileManager.default.removeItem(at: root) }
    let layout = try makeAppLayout(in: root)
    let binary = layout.helpers.appendingPathComponent("hedos")
    try Data().write(to: binary)
    let bin = root.appendingPathComponent("usr/local/bin")
    try FileManager.default.createDirectory(at: bin, withIntermediateDirectories: true)
    let link = bin.appendingPathComponent("hedos")
    try FileManager.default.createSymbolicLink(at: link, withDestinationURL: binary)
    let roots = ModuleBundleLocator.defaultRoots(
        resourceURL: bin, bundleURL: bin, executableURL: link)
    #expect(ModuleBundleLocator.locate(named: "hedos_Fake", roots: roots) != nil)
}
