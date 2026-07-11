import Foundation
import NIOCore
import NIOHTTP1

public final class GatewayResponder: @unchecked Sendable {
    private let writer: NIOAsyncChannelOutboundWriter<HTTPServerResponsePart>
    private let lock = NSLock()
    private var started = false

    init(writer: NIOAsyncChannelOutboundWriter<HTTPServerResponsePart>) {
        self.writer = writer
    }

    public var hasStarted: Bool {
        lock.lock()
        defer { lock.unlock() }
        return started
    }

    private func markStarted() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if started { return false }
        started = true
        return true
    }

    public func respond(
        status: Int, contentType: String = "application/json", body: Data,
        extraHeaders: [(String, String)] = []
    ) async throws {
        guard markStarted() else { return }
        var headers = HTTPHeaders()
        headers.add(name: "Content-Type", value: contentType)
        headers.add(name: "Content-Length", value: String(body.count))
        for (name, value) in extraHeaders {
            headers.add(name: name, value: value)
        }
        let head = HTTPResponseHead(
            version: .http1_1, status: HTTPResponseStatus(statusCode: status), headers: headers)
        try await writer.write(.head(head))
        if !body.isEmpty {
            try await writer.write(.body(.byteBuffer(ByteBuffer(bytes: body))))
        }
        try await writer.write(.end(nil))
    }

    public func beginStream(
        status: Int = 200, contentType: String
    ) async throws -> GatewayStreamBody {
        guard markStarted() else {
            throw GatewayError(.serverError, "response already started")
        }
        var headers = HTTPHeaders()
        headers.add(name: "Content-Type", value: contentType)
        headers.add(name: "Transfer-Encoding", value: "chunked")
        headers.add(name: "Cache-Control", value: "no-cache")
        let head = HTTPResponseHead(
            version: .http1_1, status: HTTPResponseStatus(statusCode: status), headers: headers)
        try await writer.write(.head(head))
        return GatewayStreamBody(writer: writer)
    }
}

public final class GatewayStreamBody: @unchecked Sendable {
    private let writer: NIOAsyncChannelOutboundWriter<HTTPServerResponsePart>
    private let lock = NSLock()
    private var ended = false

    init(writer: NIOAsyncChannelOutboundWriter<HTTPServerResponsePart>) {
        self.writer = writer
    }

    private func isEnded() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return ended
    }

    public func write(_ data: Data) async throws {
        guard !data.isEmpty, !isEnded() else { return }
        try await writer.write(.body(.byteBuffer(ByteBuffer(bytes: data))))
    }

    private func markEnded() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if ended { return false }
        ended = true
        return true
    }

    public func end() async throws {
        guard markEnded() else { return }
        try await writer.write(.end(nil))
    }
}
