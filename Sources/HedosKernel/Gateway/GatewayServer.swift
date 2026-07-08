import Foundation
import NIOCore
import NIOHTTP1
import NIOPosix

public struct GatewayStatus: Sendable, Hashable {
    public var running: Bool
    public var port: Int?

    public init(running: Bool, port: Int? = nil) {
        self.running = running
        self.port = port
    }
}

final class GatewayConnectionCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0

    func admit(limit: Int) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard count < limit else { return false }
        count += 1
        return true
    }

    func release() {
        lock.lock()
        count -= 1
        lock.unlock()
    }
}

public actor GatewayServer {
    public struct Configuration: Sendable, Hashable {
        public var port: Int
        public var maxConnections: Int
        public var maxBodyBytes: Int

        public init(port: Int = 43367, maxConnections: Int = 128, maxBodyBytes: Int = 2_097_152) {
            self.port = port
            self.maxConnections = maxConnections
            self.maxBodyBytes = maxBodyBytes
        }
    }

    private let configuration: Configuration
    private let router: GatewayRouter
    private var group: MultiThreadedEventLoopGroup?
    private var acceptTask: Task<Void, Never>?
    private var port: Int?

    public init(configuration: Configuration, router: GatewayRouter) {
        self.configuration = configuration
        self.router = router
    }

    public var boundPort: Int? { port }

    public var status: GatewayStatus {
        GatewayStatus(running: acceptTask != nil, port: port)
    }

    @discardableResult
    public func start() async throws -> Int {
        if let port, acceptTask != nil { return port }
        let group = MultiThreadedEventLoopGroup(numberOfThreads: 1)
        self.group = group
        let serverChannel: NIOAsyncChannel<
            NIOAsyncChannel<HTTPServerRequestPart, HTTPServerResponsePart>, Never
        >
        do {
            serverChannel = try await ServerBootstrap(group: group)
                .serverChannelOption(ChannelOptions.socketOption(.so_reuseaddr), value: 1)
                .serverChannelOption(ChannelOptions.backlog, value: 256)
                .childChannelOption(ChannelOptions.socketOption(.tcp_nodelay), value: 1)
                .bind(host: "127.0.0.1", port: configuration.port) { channel in
                    channel.eventLoop.makeCompletedFuture {
                        try channel.pipeline.syncOperations.configureHTTPServerPipeline()
                        return try NIOAsyncChannel(wrappingChannelSynchronously: channel)
                    }
                }
        } catch {
            try? await group.shutdownGracefully()
            self.group = nil
            throw GatewayError(
                .serverError,
                "could not bind 127.0.0.1:\(configuration.port) — \(String(describing: error))")
        }
        let bound = serverChannel.channel.localAddress?.port ?? configuration.port
        port = bound
        let router = router
        let configuration = configuration
        acceptTask = Task {
            let counter = GatewayConnectionCounter()
            try? await serverChannel.executeThenClose { inbound in
                try await withThrowingDiscardingTaskGroup { tasks in
                    for try await connection in inbound {
                        guard counter.admit(limit: configuration.maxConnections) else {
                            connection.channel.close(promise: nil)
                            continue
                        }
                        tasks.addTask {
                            defer { counter.release() }
                            try? await Self.serve(
                                connection: connection, router: router,
                                maxBodyBytes: configuration.maxBodyBytes)
                        }
                    }
                }
            }
        }
        return bound
    }

    public func stop() async {
        acceptTask?.cancel()
        _ = await acceptTask?.value
        acceptTask = nil
        port = nil
        if let group {
            try? await group.shutdownGracefully()
        }
        group = nil
    }

    private static func serve(
        connection: NIOAsyncChannel<HTTPServerRequestPart, HTTPServerResponsePart>,
        router: GatewayRouter, maxBodyBytes: Int
    ) async throws {
        try await connection.executeThenClose { inbound, outbound in
            var iterator = inbound.makeAsyncIterator()
            while let part = try await iterator.next() {
                guard case .head(let head) = part else { continue }
                var body = Data()
                var tooLarge = false
                while let next = try await iterator.next() {
                    if case .body(let buffer) = next {
                        if body.count + buffer.readableBytes > maxBodyBytes {
                            tooLarge = true
                        } else {
                            body.append(contentsOf: buffer.readableBytesView)
                        }
                    }
                    if case .end = next { break }
                }
                let responder = GatewayResponder(writer: outbound)
                if tooLarge {
                    let error = GatewayError(.badRequest, "request body exceeds \(maxBodyBytes) bytes")
                    var payload = error.body(for: GatewayRouter.surface(for: head.uri))
                    if payload.isEmpty { payload = Data("{}".utf8) }
                    try await responder.respond(status: 413, body: payload)
                    break
                }
                let request = GatewayRequest(
                    method: head.method.rawValue,
                    uri: head.uri,
                    headers: head.headers.map { ($0.name, $0.value) },
                    body: body)
                try await router.dispatch(request, responder: responder)
                if !head.isKeepAlive { break }
            }
        }
    }
}
