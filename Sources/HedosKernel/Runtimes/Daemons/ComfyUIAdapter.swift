import Foundation

struct ComfyUIAdapter: RuntimeAdapter, JobRunning {
    var id: RuntimeID { .comfyUI }

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
        let state = liveness.current().comfyUI
        guard state.alive, DaemonLiveness.matches(record: record, servedModels: state.models)
        else { return nil }
        return RuntimeBid(tier: .native, preference: BidPreference.comfyUI)
    }

    func invoke(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<CapabilityChunk, Error> {
        AsyncThrowingStream {
            $0.finish(
                throwing: KernelError.wrongExecutionMode(runtimeID: .comfyUI, expected: .job))
        }
    }

    static func checkpointName(for record: ModelRecord, servedModels: [String]) -> String {
        DaemonLiveness.matchingModels(record: record, servedModels: servedModels).first
            ?? record.name
    }

    static func graph(payload: JSONValue, checkpoint: String) -> [String: Any] {
        let object = payload.objectValue ?? [:]
        let prompt = object["prompt"]?.stringValue ?? ""
        let steps = object["steps"]?.intValue ?? 20
        let cfg = object["guidance"]?.doubleValue ?? object["cfg_scale"]?.doubleValue ?? 7
        let (width, height) = DaemonLiveness.dimensions(object)
        let seed = object["seed"]?.intValue ?? 0
        return [
            "3": [
                "class_type": "KSampler",
                "inputs": [
                    "seed": seed, "steps": steps, "cfg": cfg, "sampler_name": "euler",
                    "scheduler": "normal", "denoise": 1.0,
                    "model": ["4", 0], "positive": ["6", 0], "negative": ["7", 0],
                    "latent_image": ["5", 0],
                ] as [String: Any],
            ],
            "4": [
                "class_type": "CheckpointLoaderSimple",
                "inputs": ["ckpt_name": checkpoint],
            ],
            "5": [
                "class_type": "EmptyLatentImage",
                "inputs": ["width": width, "height": height, "batch_size": 1],
            ],
            "6": [
                "class_type": "CLIPTextEncode",
                "inputs": ["text": prompt, "clip": ["4", 1]] as [String: Any],
            ],
            "7": [
                "class_type": "CLIPTextEncode",
                "inputs": ["text": "", "clip": ["4", 1]] as [String: Any],
            ],
            "8": [
                "class_type": "VAEDecode",
                "inputs": ["samples": ["3", 0], "vae": ["4", 2]] as [String: Any],
            ],
            "9": [
                "class_type": "SaveImage",
                "inputs": ["filename_prefix": "hedos", "images": ["8", 0]] as [String: Any],
            ],
        ]
    }

    func run(
        _ record: ModelRecord, _ capability: Capability, payload: JSONValue
    ) -> AsyncThrowingStream<JobRuntimeEvent, Error> {
        AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    continuation.yield(.started)
                    let served = liveness.current().comfyUI.models
                    let checkpoint = Self.checkpointName(for: record, servedModels: served)
                    let graph = Self.graph(payload: payload, checkpoint: checkpoint)
                    let promptID = try await submit(graph)
                    let image = try await awaitResult(promptID: promptID, continuation: continuation)
                    continuation.yield(.result(data: image, fileExtension: "png"))
                    continuation.finish()
                } catch {
                    if error is URLError { liveness.markDead(.comfyUI) }
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private func submit(_ graph: [String: Any]) async throws -> String {
        var request = URLRequest(url: liveness.comfyURL.appendingPathComponent("prompt"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["prompt": graph])
        let (data, _) = try await session.data(for: request)
        guard let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let promptID = object["prompt_id"] as? String
        else { throw KernelError.runtimeFailed("ComfyUI did not return a prompt id") }
        return promptID
    }

    private func awaitResult(
        promptID: String,
        continuation: AsyncThrowingStream<JobRuntimeEvent, Error>.Continuation
    ) async throws -> Data {
        let historyURL = liveness.comfyURL.appendingPathComponent("history/\(promptID)")
        while true {
            try Task.checkCancellation()
            let (data, _) = try await session.data(from: historyURL)
            if let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                let entry = object[promptID] as? [String: Any],
                let outputs = entry["outputs"] as? [String: Any]
            {
                if let image = try await firstImage(outputs) { return image }
                throw KernelError.runtimeFailed("ComfyUI produced no image")
            }
            try await Task.sleep(for: .milliseconds(500))
        }
    }

    private func firstImage(_ outputs: [String: Any]) async throws -> Data? {
        for (_, value) in outputs {
            guard let node = value as? [String: Any],
                let images = node["images"] as? [[String: Any]],
                let first = images.first,
                let filename = first["filename"] as? String
            else { continue }
            var components = URLComponents(
                url: liveness.comfyURL.appendingPathComponent("view"),
                resolvingAgainstBaseURL: false)
            components?.queryItems = [
                URLQueryItem(name: "filename", value: filename),
                URLQueryItem(name: "subfolder", value: first["subfolder"] as? String ?? ""),
                URLQueryItem(name: "type", value: first["type"] as? String ?? "output"),
            ]
            guard let url = components?.url else { continue }
            let (data, _) = try await session.data(from: url)
            return data
        }
        return nil
    }
}
