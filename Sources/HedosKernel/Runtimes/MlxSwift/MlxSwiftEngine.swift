import Foundation
import MLX
import MLXLLM
import MLXLMCommon

actor MlxSwiftEngine {
    static let shared = MlxSwiftEngine()

    private var loadedPath: String?
    private var pendingLoadMs: Int?
    private var loadedModelID: String?
    private var container: ModelContainer?
    private let generationSlot = GenerationSlot()

    struct GenerationParams: Sendable {
        var temperature: Float
        var topP: Float?
        var repeatPenalty: Float?
        var stop: [String]
        var maxTokens: Int

        init(
            temperature: Float = 0.7, topP: Float? = nil, repeatPenalty: Float? = nil,
            stop: [String] = [], maxTokens: Int = 2048
        ) {
            self.temperature = temperature
            self.topP = topP
            self.repeatPenalty = repeatPenalty
            self.stop = stop
            self.maxTokens = maxTokens
        }
    }

    func run(
        path: String,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        governor: MemoryGovernor,
        messages: [ChatMessage],
        params: GenerationParams = GenerationParams(),
        tools: [ToolSpec] = [],
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
                try await generate(
                    messages: messages, params: params, tools: tools,
                    continuation: continuation)
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
        try await GovernedEngineLoad.acquireLoaded(
            governor: governor, producer: producer,
            modelID: modelID, modelName: modelName, footprintMB: footprintMB,
            tightStatus: "Memory is tight — generation may be slow",
            status: { continuation.yield(.status($0)) },
            isLoaded: { await self.hasLoaded(path: path) },
            previousModelID: { await self.loadedModelID },
            unloadPrevious: { await self.unload() },
            load: {
                let start = ContinuousClock.now
                try await self.load(path: path, modelID: modelID)
                await self.stampLoad(since: start)
            },
            evict: { [weak self] in await self?.unloadIfLoaded(path: path) },
            observedFootprintMB: { Footprint.directoryMB(path: path) })
    }

    private func hasLoaded(path: String) -> Bool {
        loadedPath == path && container != nil
    }

    private func generate(
        messages: [ChatMessage],
        params: GenerationParams,
        tools: [ToolSpec],
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        guard let container else {
            throw KernelError.runtimeFailed("mlx-swift model not loaded")
        }
        guard !messages.isEmpty else {
            throw KernelError.runtimeFailed("chat produced no messages")
        }
        let loadMs = pendingLoadMs
        pendingLoadMs = nil

        var parameterBuilder = GenerateParameters(
            maxTokens: params.maxTokens,
            temperature: params.temperature,
            topP: params.topP ?? 1.0)
        if let repeatPenalty = params.repeatPenalty {
            parameterBuilder.repetitionPenalty = repeatPenalty
        }
        let generateParameters = parameterBuilder

        let clock = ContinuousClock()
        let started = clock.now
        var splitter = ThinkSplitter()
        var promptTokenCount = 0
        var completionTokenCount = 0
        var sawToolCall = false
        var stopMatcher = StopMatcher(params.stop)
        var stoppedByMatch = false
        var emittedCharacters = 0

        func emitText(_ text: String) {
            guard !text.isEmpty else { return }
            guard stopMatcher.isActive else {
                continuation.yield(.text(text))
                emittedCharacters += text.count
                return
            }
            let safe = stopMatcher.feed(text)
            if !safe.isEmpty {
                continuation.yield(.text(safe))
                emittedCharacters += safe.count
            }
            if stopMatcher.stopped { stoppedByMatch = true }
        }

        let stream = try await container.perform { context -> AsyncStream<Generation> in
            let input: LMInput
            if context.tokenizer.hasChatTemplate {
                let chat: [Chat.Message] = messages.map { message in
                    switch message.role {
                    case .system: .system(message.content)
                    case .user: .user(message.content)
                    case .assistant: .assistant(message.inlinedToolTranscript.content)
                    case .tool: .tool(message.content)
                    }
                }
                let libraryTools: [[String: Any]]? =
                    tools.isEmpty
                    ? nil
                    : tools.map { spec in
                        [
                            "type": "function",
                            "function": [
                                "name": spec.name,
                                "description": spec.description,
                                "parameters": spec.parameters.anyValue,
                            ],
                        ]
                    }
                input = try await context.processor.prepare(
                    input: UserInput(chat: chat, tools: libraryTools))
            } else {
                continuation.yield(.status(ChatMLPrompt.noTemplateNotice))
                let rendered = Self.renderChatML(messages)
                input = try await context.processor.prepare(input: UserInput(prompt: .text(rendered)))
            }
            return try MLXLMCommon.generate(
                input: input, parameters: generateParameters, context: context)
        }

        consume: for await generation in stream {
            try Task.checkCancellation()
            switch generation {
            case .chunk(let text):
                for piece in splitter.feed(text) {
                    switch piece {
                    case .text(let value):
                        emitText(value)
                        if stoppedByMatch { break consume }
                    case .thinking(let value): continuation.yield(.thinking(value))
                    }
                }
            case .info(let info):
                promptTokenCount = info.promptTokenCount
                completionTokenCount = info.generationTokenCount
            case .toolCall(let call):
                sawToolCall = true
                continuation.yield(
                    .toolCall(
                        ToolCall(
                            name: call.function.name,
                            arguments: Self.kernelJSON(call.function.arguments))))
            }
        }
        if !stoppedByMatch {
            for piece in splitter.flush() {
                switch piece {
                case .text(let value): emitText(value)
                case .thinking(let value): continuation.yield(.thinking(value))
                }
            }
            if stopMatcher.isActive, !stoppedByMatch {
                let tail = stopMatcher.flush()
                if !tail.isEmpty { continuation.yield(.text(tail)) }
            }
        }

        let elapsed = clock.now - started
        let missedTerminalInfo = completionTokenCount == 0 && stoppedByMatch
        continuation.yield(
            .done(
                GenerationStats(
                    promptTokens: promptTokenCount == 0 ? nil : promptTokenCount,
                    completionTokens: missedTerminalInfo
                        ? max(1, emittedCharacters / 4) : completionTokenCount,
                    durationMs: Int(elapsed.components.seconds) * 1000
                        + Int(elapsed.components.attoseconds / 1_000_000_000_000_000),
                    loadMs: loadMs,
                    finishReason: sawToolCall
                        ? "tool_calls" : (stoppedByMatch ? "stop" : nil),
                    tokenCountsEstimated: missedTerminalInfo)))
    }

    private func stampLoad(since start: ContinuousClock.Instant) {
        pendingLoadMs = Int((ContinuousClock.now - start) / .milliseconds(1))
    }

    private func load(path: String, modelID: String) async throws {
        unload()
        let directory = URL(fileURLWithPath: path)
        let configuration = MLXLMCommon.ModelConfiguration(directory: directory)
        let loaded = try await LLMModelFactory.shared.loadContainer(configuration: configuration)
        container = loaded
        loadedPath = path
        loadedModelID = modelID
        let footprintMB = Footprint.directoryMB(path: path)
        Self.applyCacheLimit(footprintMB: footprintMB)
    }

    func unloadIfLoaded(path: String) {
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

    static func kernelJSON(_ value: MLXLMCommon.JSONValue) -> JSONValue {
        switch value {
        case .null: .null
        case .bool(let flag): .bool(flag)
        case .int(let number): .int(number)
        case .double(let number): .double(number)
        case .string(let text): .string(text)
        case .array(let values): .array(values.map(kernelJSON))
        case .object(let fields): .object(fields.mapValues(kernelJSON))
        }
    }

    static func kernelJSON(_ arguments: [String: MLXLMCommon.JSONValue]) -> JSONValue {
        .object(arguments.mapValues(kernelJSON))
    }

    static func renderChatML(_ messages: [ChatMessage]) -> String {
        ChatMLPrompt.render(messages)
    }

}
