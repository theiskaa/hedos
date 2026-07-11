import Foundation

enum StreamFailure {
    static func write(
        _ error: any Error, surface: GatewaySurface, body: GatewayStreamBody
    ) async throws {
        let gatewayError = GatewayError.wrapping(error)
        try await emit(
            message: gatewayError.message, type: gatewayError.wireType,
            code: gatewayError.wireCode, surface: surface, body: body)
    }

    static func timeout(
        surface: GatewaySurface, body: GatewayStreamBody, seconds: Int
    ) async throws {
        try await emit(
            message: "the request timed out after \(seconds)s",
            type: GatewayError.Kind.timeout.rawValue, code: "timeout", surface: surface,
            body: body)
    }

    private static func emit(
        message: String, type: String, code: String?, surface: GatewaySurface,
        body: GatewayStreamBody
    ) async throws {
        switch surface {
        case .openAI:
            var error: [String: Any] = ["message": message, "type": type]
            if let code { error["code"] = code }
            try await body.write(OpenAIWire.sseFrame(["error": error]))
            try await body.write(OpenAIWire.sseDone)
            try await body.end()
        case .ollama:
            try await body.write(OllamaWire.line(["error": message]))
            try await body.end()
        }
    }
}

enum StreamTimeout {
    static func race(
        seconds: Int, _ drain: @escaping @Sendable () async throws -> Void
    ) async throws -> Bool {
        try await withThrowingTaskGroup(of: Bool.self) { group in
            group.addTask {
                try await drain()
                return false
            }
            group.addTask {
                try await Task.sleep(for: .seconds(seconds))
                return true
            }
            let first = try await group.next()!
            group.cancelAll()
            return first
        }
    }
}
