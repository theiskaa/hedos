import Foundation

public struct GatewayError: Error, Sendable, Hashable {
    public enum Kind: String, Sendable {
        case badRequest = "invalid_request_error"
        case unauthorized = "authentication_error"
        case forbidden = "permission_error"
        case notFound = "not_found_error"
        case methodNotAllowed = "method_not_allowed"
        case notSupported = "not_supported"
        case overloaded = "overloaded"
        case timeout = "timeout_error"
        case serverError = "api_error"
    }

    public var kind: Kind
    public var message: String
    public var code: String?
    public var retryAfterSeconds: Int?

    public init(
        _ kind: Kind, _ message: String, code: String? = nil, retryAfterSeconds: Int? = nil
    ) {
        self.kind = kind
        self.message = message
        self.code = code
        self.retryAfterSeconds = retryAfterSeconds
    }

    public var status: Int {
        switch kind {
        case .badRequest: 400
        case .unauthorized: 401
        case .forbidden: 403
        case .notFound: 404
        case .methodNotAllowed: 405
        case .notSupported: 501
        case .overloaded: 503
        case .timeout: 504
        case .serverError: 500
        }
    }

    public var auditOutcome: String {
        switch kind {
        case .badRequest, .methodNotAllowed: "bad_request"
        case .unauthorized: "unauthorized"
        case .forbidden: "forbidden"
        case .notFound: "not_found"
        case .notSupported: "not_supported"
        case .overloaded: "saturated"
        case .timeout: "timeout"
        case .serverError: "error"
        }
    }

    public func body(for surface: GatewaySurface) -> Data {
        switch surface {
        case .ollama:
            let payload: [String: Any] = ["error": message]
            return (try? JSONSerialization.data(withJSONObject: payload)) ?? Data()
        case .openAI:
            var error: [String: Any] = ["message": message, "type": kind.rawValue]
            if let code { error["code"] = code }
            let payload: [String: Any] = ["error": error]
            return (try? JSONSerialization.data(withJSONObject: payload)) ?? Data()
        }
    }

    static func wrapping(_ error: any Error) -> GatewayError {
        if let gateway = error as? GatewayError { return gateway }
        if let kernel = error as? KernelError {
            switch kernel {
            case .modelNotFound, .artifactNotFound, .promptNotFound:
                return GatewayError(.notFound, kernel.errorDescription ?? "not found")
            case .capabilityUnsupported, .paramUnsupported:
                return GatewayError(
                    .badRequest, kernel.errorDescription ?? "unsupported request")
            case .contextExceeded:
                return GatewayError(
                    .badRequest, kernel.errorDescription ?? "context window exceeded",
                    code: "context_length_exceeded")
            case .notImplemented:
                return GatewayError(
                    .notSupported, kernel.errorDescription ?? "not supported",
                    code: "capability_unsupported")
            case .runtimeUnavailable:
                return GatewayError(.serverError, kernel.errorDescription ?? "runtime failed")
            case .runtimeFailed:
                return GatewayError(.serverError, "the runtime failed to complete the request")
            }
        }
        return GatewayError(.serverError, "internal error")
    }
}
