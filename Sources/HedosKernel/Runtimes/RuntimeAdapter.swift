public protocol RuntimeAdapter: Sendable {
    var id: RuntimeID { get }
    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool
    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid?
    func invoke(_ record: ModelRecord, _ capability: Capability, payload: JSONValue)
        -> AsyncThrowingStream<CapabilityChunk, Error>
    func effectiveContextWindow(for record: ModelRecord, requested: Int?) -> Int?
}

extension RuntimeAdapter {
    public func effectiveContextWindow(for record: ModelRecord, requested: Int?) -> Int? {
        nil
    }
}
