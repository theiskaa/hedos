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

public actor GatewayServer {
    public struct Configuration: Sendable, Hashable {
        public var port: Int
        public var maxConnections: Int
        public var maxBodyBytes: Int
        public var readIdleTimeout: Int

        public init(
            port: Int = GatewayDefaults.port,
            maxConnections: Int = GatewayDefaults.maxConnections,
            maxBodyBytes: Int = GatewayDefaults.maxBodyBytes,
            readIdleTimeout: Int = 60
        ) {
            self.port = port
            self.maxConnections = maxConnections
            self.maxBodyBytes = maxBodyBytes
            self.readIdleTimeout = readIdleTimeout
        }
    }

    private let configuration: Configuration
    private let router: GatewayRouter
    private var group: MultiThreadedEventLoopGroup?
    private var acceptTask: Task<Void, Never>?
    private var port: Int?
    private var startTask: Task<Int, Error>?

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
        if let startTask { return try await startTask.value }
        let task = Task { try await self.performStart() }
        startTask = task
        do {
            return try await task.value
        } catch {
            startTask = nil
            throw error
        }
    }

    private func performStart() async throws -> Int {
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
            let counter = GatewayCounter()
            try? await serverChannel.executeThenClose { inbound in
                try await withThrowingDiscardingTaskGroup { tasks in
                    for try await connection in inbound {
                        guard counter.enter(limit: configuration.maxConnections) else {
                            tasks.addTask {
                                await Self.reject503(
                                    connection, readIdleTimeout: configuration.readIdleTimeout)
                            }
                            continue
                        }
                        tasks.addTask {
                            defer { counter.exit() }
                            try? await Self.serve(
                                connection: connection, router: router,
                                maxBodyBytes: configuration.maxBodyBytes,
                                readIdleTimeout: configuration.readIdleTimeout)
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
        startTask = nil
        if let group {
            try? await group.shutdownGracefully()
        }
        group = nil
    }

    private static func serve(
        connection: NIOAsyncChannel<HTTPServerRequestPart, HTTPServerResponsePart>,
        router: GatewayRouter, maxBodyBytes: Int, readIdleTimeout: Int
    ) async throws {
        let channel = connection.channel
        try await connection.executeThenClose { inbound, outbound in
            var iterator = inbound.makeAsyncIterator()
            while true {
                var deadline = armDeadline(seconds: readIdleTimeout, channel: channel)
                var head: HTTPRequestHead?
                while let part = try await iterator.next() {
                    deadline.cancel()
                    if case .head(let value) = part {
                        head = value
                        break
                    }
                    deadline = armDeadline(seconds: readIdleTimeout, channel: channel)
                }
                deadline.cancel()
                guard let head else { break }

                let headers = head.headers.map { ($0.name, $0.value) }
                let routeLimit = router.bodyLimit(for: head.uri, default: maxBodyBytes)
                let limit =
                    await router.preauthorized(headers: headers)
                    ? routeLimit : min(routeLimit, Self.unauthenticatedBodyLimit)
                var body = Data()
                var tooLarge = false
                deadline = armDeadline(seconds: readIdleTimeout, channel: channel)
                while let next = try await iterator.next() {
                    deadline.cancel()
                    if case .body(let buffer) = next {
                        if body.count + buffer.readableBytes > limit {
                            tooLarge = true
                        } else {
                            body.append(contentsOf: buffer.readableBytesView)
                        }
                    }
                    if case .end = next { break }
                    deadline = armDeadline(seconds: readIdleTimeout, channel: channel)
                }
                deadline.cancel()

                let responder = GatewayResponder(writer: outbound)
                if tooLarge {
                    let error = GatewayError(.badRequest, "request body exceeds \(limit) bytes")
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

    static let unauthenticatedBodyLimit = 64 * 1024

    private static func armDeadline(seconds: Int, channel: Channel) -> Task<Void, Never> {
        Task {
            try? await Task.sleep(for: .seconds(seconds))
            if !Task.isCancelled { channel.close(promise: nil) }
        }
    }

    private static func reject503(
        _ connection: NIOAsyncChannel<HTTPServerRequestPart, HTTPServerResponsePart>,
        readIdleTimeout: Int
    ) async {
        let channel = connection.channel
        try? await connection.executeThenClose { inbound, outbound in
            let deadline = armDeadline(seconds: readIdleTimeout, channel: channel)
            var iterator = inbound.makeAsyncIterator()
            while let part = try await iterator.next() {
                if case .head = part { break }
            }
            deadline.cancel()
            var headers = HTTPHeaders()
            headers.add(name: "Retry-After", value: "1")
            headers.add(name: "Content-Length", value: "0")
            headers.add(name: "Connection", value: "close")
            let head = HTTPResponseHead(
                version: .http1_1, status: .serviceUnavailable, headers: headers)
            try await outbound.write(.head(head))
            try await outbound.write(.end(nil))
        }
    }
}
