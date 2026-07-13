import Foundation

struct A1111Adapter: RuntimeAdapter, JobRunning {
    var id: RuntimeID { .a1111 }

    private let liveness: DaemonLiveness
    private let session: URLSession

    init(liveness: DaemonLiveness = .shared, session: URLSession = .shared) {
        self.liveness = liveness
        self.session = session
    }

    func canServe(_ record: ModelRecord, _ capability: Capability) -> Bool {
        record.runtime.id == id && capability == .image
    }

    func bid(_ record: ModelRecord, _ identified: IdentifiedModel) -> RuntimeBid? {
        guard identified.format == .diffusers,
            identified.capabilities.contains(.image)
        else { return nil }
        let state = liveness.current().a1111
        guard state.alive, DaemonLiveness.matches(record: record, servedModels: state.models)
        else { return nil }
        return RuntimeBid(tier: .native, preference: BidPreference.a1111)
    }

    static func requestBody(payload: JSONValue) -> [String: Any] {
        let object = payload.objectValue ?? [:]
        let (width, height) = DaemonLiveness.dimensions(object)
        return [
            "prompt": object["prompt"]?.stringValue ?? "",
            "steps": object["steps"]?.intValue ?? 20,
            "cfg_scale": object["guidance"]?.doubleValue ?? object["cfg_scale"]?.doubleValue ?? 7,
            "width": width,
            "height": height,
            "seed": object["seed"]?.intValue ?? -1,
        ]
    }

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    continuation.yield(.started)
                    var request = URLRequest(
                        url: liveness.a1111URL.appendingPathComponent("sdapi/v1/txt2img"))
                    request.httpMethod = "POST"
                    request.setValue("application/json", forHTTPHeaderField: "Content-Type")
                    request.httpBody = try JSONSerialization.data(
                        withJSONObject: Self.requestBody(payload: payload))
                    let (data, _) = try await session.data(for: request)
                    guard
                        let object = try JSONSerialization.jsonObject(with: data)
                            as? [String: Any],
                        let images = object["images"] as? [String],
                        let encoded = images.first,
                        let bytes = Data(base64Encoded: encoded)
                    else {
                        throw KernelError.runtimeFailed("A1111 returned no image")
                    }
                    continuation.yield(.result(data: bytes, fileExtension: "png"))
                    continuation.finish()
                } catch {
                    if error is URLError { liveness.markDead(.a1111) }
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }
}
