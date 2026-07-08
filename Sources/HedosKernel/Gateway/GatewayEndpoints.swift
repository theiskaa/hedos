import Foundation

public struct GatewayEndpointInfo: Sendable, Hashable, Identifiable {
    public var method: String
    public var path: String
    public var group: String
    public var summary: String
    public var streaming: Bool

    public var id: String { "\(method) \(path)" }

    public init(
        method: String, path: String, group: String, summary: String, streaming: Bool
    ) {
        self.method = method
        self.path = path
        self.group = group
        self.summary = summary
        self.streaming = streaming
    }
}

public enum GatewayEndpoints {
    public static let groupOrder = ["OpenAI", "Ollama", "Pipelines"]

    public static var catalog: [GatewayEndpointInfo] {
        GatewayRouter.standardRoutes().map { route in
            GatewayEndpointInfo(
                method: route.method, path: route.path, group: route.group,
                summary: route.summary, streaming: route.inference)
        }
        .sorted {
            let left = groupOrder.firstIndex(of: $0.group) ?? groupOrder.count
            let right = groupOrder.firstIndex(of: $1.group) ?? groupOrder.count
            if left != right { return left < right }
            return $0.path < $1.path
        }
    }

    public static var grouped: [(group: String, endpoints: [GatewayEndpointInfo])] {
        var seen: [String] = []
        for endpoint in catalog where !seen.contains(endpoint.group) {
            seen.append(endpoint.group)
        }
        return seen.map { group in
            (group, catalog.filter { $0.group == group })
        }
    }
}
