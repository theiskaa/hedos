import Foundation

extension RuntimeAdapter where Self: JobRunning {
    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream {
            $0.finish(throwing: KernelError.wrongExecutionMode(runtimeID: id, expected: .job))
        }
    }
}
