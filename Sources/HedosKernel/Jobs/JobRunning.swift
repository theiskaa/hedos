public protocol JobRunning: Sendable {
    func run(_ record: ModelRecord, _ capability: Capability, payload: JSONValue)
        -> AsyncThrowingStream<JobRuntimeEvent, Error>
}
