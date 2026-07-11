import Foundation
import Testing

@testable import HedosKernel

@Test func vmBufferWriterAccumulatesUnderTheCap() throws {
    let writer = VMBufferWriter(maxBytes: 100)
    try writer.write(Data(repeating: 0x41, count: 40))
    try writer.write(Data(repeating: 0x42, count: 40))
    #expect(writer.text.count == 80)
}

@Test func vmBufferWriterThrowsWhenACrossingWriteWouldExceedTheCap() throws {
    let writer = VMBufferWriter(maxBytes: 100)
    try writer.write(Data(repeating: 0x41, count: 80))
    #expect(throws: KernelError.self) {
        try writer.write(Data(repeating: 0x42, count: 40))
    }
    #expect(writer.text.count == 80)
}
