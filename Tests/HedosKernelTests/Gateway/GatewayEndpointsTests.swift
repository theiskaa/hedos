import Foundation
import Testing

@testable import HedosKernel

@Test func endpointCatalogCoversEveryServedRoute() {
    let routePaths = Set(GatewayRouter.standardRoutes().map { "\($0.method) \($0.path)" })
    let catalogPaths = Set(GatewayEndpoints.catalog.map(\.id))
    #expect(routePaths == catalogPaths)
}

@Test func everyEndpointHasGroupAndSummary() {
    for endpoint in GatewayEndpoints.catalog {
        #expect(!endpoint.group.isEmpty, "\(endpoint.path) has no group")
        #expect(!endpoint.summary.isEmpty, "\(endpoint.path) has no summary")
        #expect(GatewayEndpoints.groupOrder.contains(endpoint.group))
    }
}

@Test func groupedEndpointsAreOrderedAndComplete() {
    let grouped = GatewayEndpoints.grouped
    #expect(grouped.map(\.group) == ["OpenAI", "Ollama"])
    let flattened = grouped.flatMap { $0.endpoints }
    #expect(flattened.count == GatewayEndpoints.catalog.count)
    #expect(grouped.contains { $0.endpoints.contains { $0.path == "/v1/chat/completions" } })
}

@Test func streamingFlagMatchesInferenceRoutes() {
    let streaming = Set(
        GatewayEndpoints.catalog.filter(\.streaming).map(\.path))
    #expect(streaming.contains("/v1/chat/completions"))
    #expect(streaming.contains("/v1/audio/speech"))
    #expect(!streaming.contains("/v1/models"))
    #expect(!streaming.contains("/api/tags"))
}
