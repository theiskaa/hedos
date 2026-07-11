import Foundation

enum GatewayParamGuard {
    static let structuralKeys: Set<String> = ["thinking"]

    static func require(
        _ params: [String: JSONValue], honoredBy honored: Set<String>, runtime: RuntimeID?
    ) throws {
        for key in params.keys.sorted() where !structuralKeys.contains(key) {
            guard honored.contains(key) else {
                let runtimeName = runtime?.rawValue ?? "selected"
                throw GatewayError(
                    .badRequest,
                    "the parameter '\(key)' is not supported by the \(runtimeName) runtime serving this model",
                    code: "unsupported_parameter")
            }
        }
    }
}
