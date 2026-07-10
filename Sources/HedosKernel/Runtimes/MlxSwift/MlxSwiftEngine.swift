import Foundation
import MLX
import MLXLLM
import MLXLMCommon

public actor MlxSwiftEngine {
    public static let shared = MlxSwiftEngine()

    private var loadedPath: String?
    private var loadedModelID: String?
    private var container: ModelContainer?
    private let generationSlot = GenerationSlot()

    public struct GenerationParams: Sendable {
        public var temperature: Float
        public var topP: Float?
        public var maxTokens: Int

        public init(temperature: Float = 0.7, topP: Float? = nil, maxTokens: Int = 2048) {
            self.temperature = temperature
            self.topP = topP
            self.maxTokens = maxTokens
        }
    }

    public func run(
        path: String,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        governor: MemoryGovernor,
        messages: [ChatMessage],
        params: GenerationParams = GenerationParams(),
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async {
        await governor.beginGeneration(modelID)
        do {
            let producer = GPUProducer.generation(modelID: modelID)
            try await acquireGateWithModelLoaded(
                producer: producer, path: path, modelID: modelID, modelName: modelName,
                footprintMB: footprintMB, governor: governor, continuation: continuation)
            await generationSlot.acquire()
            do {
                try await generate(messages: messages, params: params, continuation: continuation)
                MLX.Stream().synchronize()
                await generationSlot.release()
                await governor.gate.release(producer)
            } catch {
                MLX.Stream().synchronize()
                await generationSlot.release()
                await governor.gate.release(producer)
                throw error
            }
            continuation.finish()
        } catch {
            continuation.finish(throwing: error)
        }
        await governor.endGeneration(modelID)
    }

    private func acquireGateWithModelLoaded(
        producer: GPUProducer,
        path: String,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        governor: MemoryGovernor,
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        while true {
            try await ensureLoadedGoverned(
                path: path, modelID: modelID, modelName: modelName,
                footprintMB: footprintMB, governor: governor, continuation: continuation)
            await governor.gate.acquire(producer)
            if loadedPath == path, container != nil { return }
            await governor.gate.release(producer)
        }
    }

    private func generate(
        messages: [ChatMessage],
        params: GenerationParams,
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        guard let container else {
            throw KernelError.runtimeFailed("mlx-swift model not loaded")
        }
        guard !messages.isEmpty else {
            throw KernelError.runtimeFailed("chat produced no messages")
        }

        let generateParameters = GenerateParameters(
            maxTokens: params.maxTokens,
            temperature: params.temperature,
            topP: params.topP ?? 1.0)

        let clock = ContinuousClock()
        let started = clock.now
        var splitter = ThinkSplitter()
        var promptTokenCount = 0
        var completionTokenCount = 0

        let stream = try await container.perform { context -> AsyncStream<Generation> in
            let input: LMInput
            if context.tokenizer.hasChatTemplate {
                let chat: [Chat.Message] = messages.map { message in
                    switch message.role {
                    case .system: .system(message.content)
                    case .user: .user(message.content)
                    case .assistant: .assistant(message.content)
                    }
                }
                input = try await context.processor.prepare(input: UserInput(chat: chat))
            } else {
                let rendered = Self.renderChatML(messages)
                input = try await context.processor.prepare(input: UserInput(prompt: .text(rendered)))
            }
            return try MLXLMCommon.generate(
                input: input, parameters: generateParameters, context: context)
        }

        for await generation in stream {
            try Task.checkCancellation()
            switch generation {
            case .chunk(let text):
                for piece in splitter.feed(text) {
                    switch piece {
                    case .text(let value): continuation.yield(.text(value))
                    case .thinking(let value): continuation.yield(.thinking(value))
                    }
                }
            case .info(let info):
                promptTokenCount = info.promptTokenCount
                completionTokenCount = info.generationTokenCount
            case .toolCall:
                break
            }
        }
        for piece in splitter.flush() {
            switch piece {
            case .text(let value): continuation.yield(.text(value))
            case .thinking(let value): continuation.yield(.thinking(value))
            }
        }

        let elapsed = clock.now - started
        continuation.yield(
            .done(
                GenerationStats(
                    promptTokens: promptTokenCount,
                    completionTokens: completionTokenCount,
                    durationMs: Int(elapsed.components.seconds) * 1000
                        + Int(elapsed.components.attoseconds / 1_000_000_000_000_000))))
    }

    private func ensureLoadedGoverned(
        path: String,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        governor: MemoryGovernor,
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        if loadedPath == path, container != nil { return }
        let verdict = try await governor.admit(
            modelID: modelID, name: modelName, footprintMB: footprintMB
        ) { reason in
            continuation.yield(.status(reason))
        }
        if verdict == .tight {
            continuation.yield(.status("Memory is tight — generation may be slow"))
        }
        let producer = GPUProducer.load(modelID: modelID)
        await governor.gate.acquire(producer)
        do {
            if let previousModelID = loadedModelID {
                unload()
                await governor.markUnloaded(previousModelID)
            }
            try await load(path: path, modelID: modelID)
            await governor.gate.release(producer)
        } catch {
            await governor.gate.release(producer)
            await governor.markUnloaded(modelID)
            throw error
        }
        await governor.markLoaded(
            modelID: modelID, name: modelName, footprintMB: footprintMB
        ) {
            await MlxSwiftEngine.shared.unloadIfLoaded(path: path)
        }
        if let observed = Self.directoryFootprintMB(path: path) {
            await governor.observeFootprint(modelID, footprintMB: observed)
        }
    }

    private func load(path: String, modelID: String) async throws {
        unload()
        let directory = URL(fileURLWithPath: path)
        let configuration = MLXLMCommon.ModelConfiguration(directory: directory)
        let loaded = try await LLMModelFactory.shared.loadContainer(configuration: configuration)
        container = loaded
        loadedPath = path
        loadedModelID = modelID
        let footprintMB = Self.directoryFootprintMB(path: path)
        Self.applyCacheLimit(footprintMB: footprintMB)
    }

    public func unloadIfLoaded(path: String) {
        guard loadedPath == path else { return }
        unload()
    }

    private func unload() {
        container = nil
        loadedPath = nil
        loadedModelID = nil
        MLX.GPU.clearCache()
    }

    static func cacheLimit(footprintMB: Int?) -> Int {
        let gib = 1024 * 1024 * 1024
        let footprintBytes = (footprintMB ?? 0) * (1 << 20)
        let weightsBased = max(footprintBytes / 4, gib)
        let ramBased = min(Int(ProcessInfo.processInfo.physicalMemory) / 8, 8 * gib)
        return min(weightsBased, ramBased)
    }

    private static func applyCacheLimit(footprintMB: Int?) {
        MLX.GPU.set(cacheLimit: cacheLimit(footprintMB: footprintMB))
    }

    static func renderChatML(_ messages: [ChatMessage]) -> String {
        var rendered = ""
        for message in messages {
            rendered += "<|im_start|>\(message.role.rawValue)\n\(message.content)<|im_end|>\n"
        }
        rendered += "<|im_start|>assistant\n"
        return rendered
    }

    static func directoryFootprintMB(path: String) -> Int? {
        let url = URL(fileURLWithPath: path)
        guard
            let enumerator = FileManager.default.enumerator(
                at: url, includingPropertiesForKeys: [.fileSizeKey, .isRegularFileKey])
        else { return nil }
        var total = 0
        for case let entry as URL in enumerator {
            let values = try? entry.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey])
            if values?.isRegularFile == true { total += values?.fileSize ?? 0 }
        }
        return total / (1 << 20)
    }
}
