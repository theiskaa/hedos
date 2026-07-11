import Foundation

struct CappedLineReader<Bytes: AsyncSequence & Sendable>: Sendable
where Bytes.Element == UInt8 {
    let bytes: Bytes
    let maxLineBytes: Int
    let maxResponseBytes: Int
    let source: String

    init(
        bytes: Bytes, source: String,
        maxLineBytes: Int = OpenAIEndpointAdapter.defaultMaxLineBytes,
        maxResponseBytes: Int = OpenAIEndpointAdapter.defaultMaxResponseBytes
    ) {
        self.bytes = bytes
        self.source = source
        self.maxLineBytes = maxLineBytes
        self.maxResponseBytes = maxResponseBytes
    }

    func lines() -> AsyncThrowingStream<String, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var lineBuffer: [UInt8] = []
                    var total = 0
                    for try await byte in bytes {
                        if Task.isCancelled { break }
                        total += 1
                        if total > maxResponseBytes {
                            throw KernelError.runtimeFailed(
                                "\(source) sent a response larger than \(maxResponseBytes) bytes")
                        }
                        if byte == 0x0A {
                            continuation.yield(String(decoding: lineBuffer, as: UTF8.self))
                            lineBuffer.removeAll(keepingCapacity: true)
                        } else {
                            lineBuffer.append(byte)
                            if lineBuffer.count > maxLineBytes {
                                throw KernelError.runtimeFailed(
                                    "\(source) sent a line larger than \(maxLineBytes) bytes")
                            }
                        }
                    }
                    if !lineBuffer.isEmpty {
                        continuation.yield(String(decoding: lineBuffer, as: UTF8.self))
                    }
                    continuation.finish()
                } catch where Task.isCancelled {
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
