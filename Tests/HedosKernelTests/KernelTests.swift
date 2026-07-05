import Testing

@testable import HedosKernel

@Test func kernelIsConstructibleAndVersioned() {
    _ = Kernel()
    #expect(!Kernel.version.isEmpty)
}
