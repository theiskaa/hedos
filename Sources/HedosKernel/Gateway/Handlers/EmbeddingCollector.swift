import Foundation

enum EmbeddingCollector {
    static func collect(
        record: ModelRecord, inputs: [String], port: any GatewayPort
    ) async throws -> (vectors: [[Double]], stats: GenerationStats?) {
        let inputPayload: JSONValue =
            inputs.count == 1 ? .string(inputs[0]) : .array(inputs.map(JSONValue.string))
        do {
            let stream = try await port.invoke(
                record.id, .embed, payload: .object(["input": inputPayload]))
            var vectors: [[Double]] = []
            var finalStats: GenerationStats?
            for try await chunk in stream {
                switch chunk {
                case .vector(let vector):
                    vectors.append(vector)
                case .done(let stats):
                    finalStats = stats
                case .text, .thinking, .audio, .status, .toolCall, .segment:
                    break
                }
            }
            guard !vectors.isEmpty else {
                throw GatewayError(.serverError, "\(record.name) produced no embeddings")
            }
            guard vectors.count == inputs.count else {
                throw GatewayError(
                    .serverError,
                    "\(record.name) returned \(vectors.count) embeddings for \(inputs.count) inputs")
            }
            return (vectors, finalStats)
        } catch KernelError.capabilityUnsupported {
            throw GatewayError(
                .notSupported, "\(record.name) has no embeddings runtime on this machine",
                code: "capability_unsupported")
        }
    }
}
