public protocol RuntimeAdapter: Sendable {
    var id: String { get }
    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool
    func invoke(_ record: ModelRecord, _ capability: Capability, payload: JSONValue)
        -> AsyncThrowingStream<CapabilityChunk, Error>
}
