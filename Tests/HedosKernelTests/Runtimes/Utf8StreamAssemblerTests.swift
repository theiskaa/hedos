import Testing

@testable import HedosKernel

@Test func multibyteScalarSplitAcrossPiecesDecodesIntact() {
    var assembler = Utf8StreamAssembler()
    var output = ""
    for byte in Array("好😀".utf8) {
        output += assembler.feed([byte])
    }
    output += assembler.flush()
    #expect(output == "好😀")
}

@Test func invalidByteBecomesReplacementAndStreamContinues() {
    var assembler = Utf8StreamAssembler()
    let output = assembler.feed([0x41, 0xFF, 0x42])
    #expect(output == "A\u{FFFD}B")
    #expect(assembler.feed(Array("ok".utf8)) == "ok")
}

@Test func flushEmitsHeldTail() {
    var assembler = Utf8StreamAssembler()
    #expect(assembler.feed([0xE5, 0xA5]) == "")
    let flushed = assembler.flush()
    #expect(!flushed.isEmpty)
    #expect(flushed.unicodeScalars.allSatisfy { $0 == "\u{FFFD}" })
    #expect(assembler.flush() == "")
}

@Test func pureASCIIPassesThroughUnbuffered() {
    var assembler = Utf8StreamAssembler()
    #expect(assembler.feed(Array("hello".utf8)) == "hello")
    #expect(assembler.flush() == "")
}

@Test func oversizedPieceRegrowsBufferAndRetries() {
    var buffer = [CChar](repeating: 0, count: 4)
    let bytes = Array("0123456789".utf8CString.dropLast())
    let written = LlamaEngine.renderPiece(into: &buffer) { pointer, capacity in
        guard capacity >= bytes.count else { return Int32(-bytes.count) }
        for (offset, byte) in bytes.enumerated() {
            pointer[offset] = byte
        }
        return Int32(bytes.count)
    }
    #expect(written == 10)
    #expect(buffer.count == 10)
    #expect(buffer[0..<10].elementsEqual(bytes))
}

@Test func stillFailingRetrySkipsThePiece() {
    var buffer = [CChar](repeating: 0, count: 4)
    let written = LlamaEngine.renderPiece(into: &buffer) { _, _ in -64 }
    #expect(written == nil)
}

@Test func emptyPieceSkipsWithoutRegrowth() {
    var buffer = [CChar](repeating: 0, count: 4)
    let written = LlamaEngine.renderPiece(into: &buffer) { _, _ in 0 }
    #expect(written == nil)
    #expect(buffer.count == 4)
}

@Test func splitAcrossUnevenChunksDecodesIntact() {
    var assembler = Utf8StreamAssembler()
    let bytes = Array("日本語です😀".utf8)
    var output = ""
    for chunk in stride(from: 0, to: bytes.count, by: 5) {
        output += assembler.feed(bytes[chunk..<min(chunk + 5, bytes.count)])
    }
    output += assembler.flush()
    #expect(output == "日本語です😀")
}
