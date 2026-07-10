import Foundation
import Testing

@testable import HedosKernel

final class InvokeRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var invocations: [(modelID: String, capability: Capability, payload: JSONValue)] = []

    func record(_ modelID: String, _ capability: Capability, _ payload: JSONValue) {
        lock.lock()
        invocations.append((modelID, capability, payload))
        lock.unlock()
    }

    var all: [(modelID: String, capability: Capability, payload: JSONValue)] {
        lock.lock()
        defer { lock.unlock() }
        return invocations
    }

    var last: (modelID: String, capability: Capability, payload: JSONValue)? {
        all.last
    }
}

struct FakeGatewayPort: GatewayPort {
    var records: [ModelRecord] = []
    var chatScript: [CapabilityChunk] = []
    var speakScript: [CapabilityChunk] = []
    var embedScript: [CapabilityChunk] = []
    var voicesList: [String] = []
    var jobResult: [String] = []
    var jobFailure: String?
    var artifacts: [String: Data] = [:]
    var admission: GatewayAdmissionState = .ready
    var pipelinesList: [Pipeline] = []
    var pipelineEventScript: [PipelineEvent] = []
    var pipelineHangs = false
    var recorder = InvokeRecorder()

    func shelf() async throws -> [ModelRecord] { records }

    func invoke(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error> {
        recorder.record(modelID, capability, payload)
        if let record = records.first(where: { $0.id == modelID }),
            !record.capabilities.contains(capability)
        {
            return AsyncThrowingStream { continuation in
                continuation.finish(
                    throwing: KernelError.capabilityUnsupported(
                        model: modelID, capability: capability))
            }
        }
        let script: [CapabilityChunk]
        switch capability {
        case .speak: script = speakScript
        case .embed: script = embedScript
        default: script = chatScript
        }
        guard !script.isEmpty else {
            return AsyncThrowingStream { continuation in
                let error: Error =
                    capability == .embed
                    ? KernelError.capabilityUnsupported(model: modelID, capability: capability)
                    : KernelError.modelNotFound(modelID)
                continuation.finish(throwing: error)
            }
        }
        return AsyncThrowingStream { continuation in
            for chunk in script {
                continuation.yield(chunk)
            }
            continuation.finish()
        }
    }

    func submit(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> String {
        recorder.record(modelID, capability, payload)
        guard !jobResult.isEmpty || jobFailure != nil else {
            throw KernelError.modelNotFound(modelID)
        }
        return "job-fake"
    }

    func job(id: String) async throws -> Job? { nil }

    func jobEvents(id: String) async -> AsyncStream<JobEvent> {
        let result = jobResult
        let failure = jobFailure
        return AsyncStream { continuation in
            continuation.yield(.running)
            if let failure {
                continuation.yield(.failed(message: failure))
            } else {
                continuation.yield(.progress(JobProgress(step: 1, totalSteps: 2)))
                continuation.yield(.done(result: result))
            }
            continuation.finish()
        }
    }

    func cancel(jobID: String) async {}

    func voices(for modelID: String) async throws -> [String] { voicesList }

    func artifactData(id: String) async throws -> Data? { artifacts[id] }

    func admissionState(
        modelID: String, footprintMB: Int?, kind: GatewayWorkKind
    ) async -> GatewayAdmissionState {
        admission
    }

    func pipelines() async -> [Pipeline] { pipelinesList }

    func pipeline(id: String) async -> Pipeline? {
        pipelinesList.first { $0.id == id }
    }

    func runPipeline(id: String, input: PipelineInput) async throws
        -> AsyncStream<PipelineEvent>
    {
        let script = pipelineEventScript
        guard pipelineHangs else {
            return AsyncStream { continuation in
                for event in script { continuation.yield(event) }
                continuation.finish()
            }
        }
        return AsyncStream { continuation in
            let task = Task {
                try? await Task.sleep(for: .seconds(3600))
                continuation.finish()
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}

struct GatewayStack {
    let server: GatewayServer
    let port: Int
    let token: String
    let clients: GatewayClientStore
    let audit: GatewayAuditLog
    let directory: URL

    func url(_ path: String) -> String {
        "http://127.0.0.1:\(port)\(path)"
    }

    func stop() async {
        await server.stop()
        try? FileManager.default.removeItem(at: directory)
    }
}

enum GatewayHarness {
    static func stack(
        port gatewayPort: any GatewayPort = FakeGatewayPort(),
        routes: [GatewayRoute] = [],
        scopes: GatewayScopes = .all,
        configuration: GatewayServer.Configuration = GatewayServer.Configuration(port: 0),
        maxConcurrentInference: Int = 4
    ) async throws -> GatewayStack {
        let directory = try Fixtures.tempDirectory()
        let clients = GatewayClientStore(directory: directory, secrets: InMemorySecretStore())
        let audit = GatewayAuditLog(directory: directory)
        let creation = try await clients.create(name: "test-client", scopes: scopes)
        let router = GatewayRouter(
            port: gatewayPort, auth: GatewayAuth(clients: clients), audit: audit, routes: routes,
            maxConcurrentInference: maxConcurrentInference)
        let server = GatewayServer(configuration: configuration, router: router)
        let boundPort = try await server.start()
        return GatewayStack(
            server: server, port: boundPort, token: creation.token, clients: clients,
            audit: audit, directory: directory)
    }

    static func request(
        _ method: String, _ url: String, token: String? = nil, body: Data? = nil
    ) -> URLRequest {
        var request = URLRequest(url: URL(string: url)!)
        request.httpMethod = method
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        if let body {
            request.httpBody = body
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        return request
    }

    static func json(_ object: [String: Any]) -> Data {
        try! JSONSerialization.data(withJSONObject: object)
    }
}

struct EchoHandler: GatewayHandling {
    var surface: GatewaySurface { .openAI }

    func handle(
        _ request: GatewayRequest, identity: GatewayIdentity, port: any GatewayPort,
        responder: GatewayResponder
    ) async throws -> GatewayOutcome {
        try await responder.respond(status: 200, body: Data("{\"ok\":true}".utf8))
        return .ok
    }
}
