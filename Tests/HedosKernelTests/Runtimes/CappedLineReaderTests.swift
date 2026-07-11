import Foundation
import Testing

@testable import HedosKernel

private func byteStream(_ chunks: [[UInt8]]) -> AsyncStream<UInt8> {
    AsyncStream { continuation in
        for chunk in chunks {
            for byte in chunk { continuation.yield(byte) }
        }
        continuation.finish()
    }
}

private func bytes(_ string: String) -> [UInt8] {
    Array(string.utf8)
}

@Test func cappedReaderSplitsLinesAcrossChunkBoundaries() async throws {
    let stream = byteStream([bytes("hel"), bytes("lo\nwor"), bytes("ld\n")])
    let reader = CappedLineReader(bytes: stream, source: "test")
    var lines: [String] = []
    for try await line in reader.lines() { lines.append(line) }
    #expect(lines == ["hello", "world"])
}

@Test func cappedReaderDeliversFinalUnterminatedLine() async throws {
    let stream = byteStream([bytes("one\ntwo")])
    let reader = CappedLineReader(bytes: stream, source: "test")
    var lines: [String] = []
    for try await line in reader.lines() { lines.append(line) }
    #expect(lines == ["one", "two"])
}

@Test func cappedReaderThrowsOnOversizedLine() async throws {
    let stream = byteStream([[UInt8](repeating: 0x41, count: 50)])
    let reader = CappedLineReader(
        bytes: stream, source: "test", maxLineBytes: 10, maxResponseBytes: 1000)
    await #expect(throws: KernelError.self) {
        for try await _ in reader.lines() {}
    }
}

@Test func cappedReaderThrowsOnOversizedTotal() async throws {
    let stream = byteStream([bytes("a\nb\nc\nd\ne\n")])
    let reader = CappedLineReader(
        bytes: stream, source: "test", maxLineBytes: 1000, maxResponseBytes: 5)
    await #expect(throws: KernelError.self) {
        for try await _ in reader.lines() {}
    }
}
