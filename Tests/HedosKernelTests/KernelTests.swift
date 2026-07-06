import Foundation
import Testing

@testable import HedosKernel

@Test func kernelIsConstructibleAndVersioned() throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    _ = Kernel(directory: dir)
    #expect(!Kernel.version.isEmpty)
}
