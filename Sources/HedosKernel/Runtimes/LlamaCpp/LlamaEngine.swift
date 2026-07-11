import Foundation
import LlamaSwift

actor LlamaEngine {
    static let shared = LlamaEngine()

    private static let backendReady: Void = {
        llama_log_set({ _, _, _ in }, nil)
        llama_backend_init()
    }()

    private var loadedPath: String?
    private var loadedModelID: String?
    private var loadedContextTokens: Int?
    private var pendingLoadMs: Int?
    private var model: OpaquePointer?
    private var context: OpaquePointer?
    private let generationSlot = GenerationSlot()

    struct GenerationParams: Sendable {
        var temperature: Float
        var topP: Float?
        var topK: Int32?
        var minP: Float?
        var repeatPenalty: Float?
        var frequencyPenalty: Float?
        var presencePenalty: Float?
        var penaltyLastN: Int32
        var stop: [String]
        var seed: UInt32?
        var jsonGrammar: String?
        var maxTokens: Int
        var tools: [ToolSpec]

        init(
            temperature: Float = 0.7, topP: Float? = nil, topK: Int32? = nil,
            minP: Float? = nil, repeatPenalty: Float? = nil, frequencyPenalty: Float? = nil,
            presencePenalty: Float? = nil, penaltyLastN: Int32 = 64, stop: [String] = [],
            seed: UInt32? = nil, jsonGrammar: String? = nil, maxTokens: Int = 2048,
            tools: [ToolSpec] = []
        ) {
            self.temperature = temperature
            self.topP = topP
            self.topK = topK
            self.minP = minP
            self.repeatPenalty = repeatPenalty
            self.frequencyPenalty = frequencyPenalty
            self.presencePenalty = presencePenalty
            self.penaltyLastN = penaltyLastN
            self.stop = stop
            self.seed = seed
            self.jsonGrammar = jsonGrammar
            self.maxTokens = maxTokens
            self.tools = tools
        }
    }

    func run(
        path: String,
        modelID: String,
        modelName: String,
        footprintMB: Int?,
        contextTokens: Int = 4096,
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
                footprintMB: footprintMB, contextTokens: contextTokens, governor: governor,
                continuation: continuation)
            await generationSlot.acquire()
            do {
                try await generate(
                    modelName: modelName, contextTokens: contextTokens,
                    messages: messages, params: params, continuation: continuation)
                await generationSlot.release()
                await governor.gate.release(producer)
            } catch {
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
        contextTokens: Int,
        governor: MemoryGovernor,
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        try await GovernedEngineLoad.acquireLoaded(
            governor: governor, producer: producer,
            modelID: modelID, modelName: modelName, footprintMB: footprintMB,
            tightStatus: "Memory is tight — generation may be slow",
            status: { continuation.yield(.status($0)) },
            isLoaded: { await self.hasLoaded(path: path, contextTokens: contextTokens) },
            previousModelID: { await self.loadedModelID },
            unloadPrevious: { await self.unload() },
            load: {
                let start = ContinuousClock.now
                try await self.load(path: path, modelID: modelID, contextTokens: contextTokens)
                await self.stampLoad(since: start)
            },
            evict: { [weak self] in await self?.unloadIfLoaded(path: path) },
            observedFootprintMB: { Footprint.weightsMB(path: path) })
    }

    private func hasLoaded(path: String, contextTokens: Int) -> Bool {
        loadedPath == path && loadedContextTokens == contextTokens
            && model != nil && context != nil
    }

    private func generate(
        modelName: String,
        contextTokens: Int,
        messages: [ChatMessage],
        params: GenerationParams,
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async throws {
        guard let model, let context else {
            throw KernelError.runtimeFailed("llama model not loaded")
        }
        let loadMs = pendingLoadMs
        pendingLoadMs = nil
        let vocab = llama_model_get_vocab(model)
        let rendered = renderPrompt(
            model: model,
            messages: Self.messagesWithToolBlock(
                messages.map(\.inlinedToolTranscript), tools: params.tools))
        if rendered.fellBack {
            continuation.yield(.status(ChatMLPrompt.noTemplateNotice))
        }
        let prompt = rendered.prompt
        var tokens = tokenize(vocab: vocab, text: prompt)
        guard !tokens.isEmpty else {
            throw KernelError.runtimeFailed("prompt produced no tokens")
        }
        guard tokens.count < contextTokens else {
            throw KernelError.contextExceeded(model: modelName)
        }

        let memory = llama_get_memory(context)
        llama_memory_clear(memory, true)

        let clock = ContinuousClock()
        let started = clock.now
        let promptCount = tokens.count

        var batch = llama_batch_get_one(&tokens, Int32(tokens.count))
        guard llama_decode(context, batch) == 0 else {
            throw KernelError.runtimeFailed("llama prompt decode failed")
        }

        let samplerParams = llama_sampler_chain_default_params()
        let sampler = llama_sampler_chain_init(samplerParams)
        if !params.tools.isEmpty, let grammar = try? ToolGrammar.grammar(for: params.tools) {
            ToolGrammar.triggerPattern.withCString { pattern in
                var patterns: [UnsafePointer<CChar>?] = [pattern]
                if let grammarSampler = llama_sampler_init_grammar_lazy_patterns(
                    vocab, grammar, "root", &patterns, 1, nil, 0)
                {
                    llama_sampler_chain_add(sampler, grammarSampler)
                }
            }
        } else if params.tools.isEmpty, let grammar = params.jsonGrammar {
            if let grammarSampler = llama_sampler_init_grammar(vocab, grammar, "root") {
                llama_sampler_chain_add(sampler, grammarSampler)
            }
        }
        if params.repeatPenalty != nil || params.frequencyPenalty != nil
            || params.presencePenalty != nil
        {
            llama_sampler_chain_add(
                sampler,
                llama_sampler_init_penalties(
                    params.penaltyLastN, params.repeatPenalty ?? 1.0,
                    params.frequencyPenalty ?? 0.0, params.presencePenalty ?? 0.0))
        }
        if params.temperature <= 0 {
            llama_sampler_chain_add(sampler, llama_sampler_init_greedy())
        } else {
            if let topK = params.topK {
                llama_sampler_chain_add(sampler, llama_sampler_init_top_k(topK))
            }
            if let topP = params.topP, topP < 1 {
                llama_sampler_chain_add(sampler, llama_sampler_init_top_p(topP, 1))
            }
            if let minP = params.minP {
                llama_sampler_chain_add(sampler, llama_sampler_init_min_p(minP, 1))
            }
            llama_sampler_chain_add(sampler, llama_sampler_init_temp(params.temperature))
            llama_sampler_chain_add(
                sampler,
                llama_sampler_init_dist(params.seed ?? UInt32.random(in: 0..<UInt32.max)))
        }
        defer { llama_sampler_free(sampler) }

        var generated = 0
        let maxTokens = min(params.maxTokens, contextTokens - promptCount)
        var pieceBuffer = [CChar](repeating: 0, count: 512)
        var assembler = Utf8StreamAssembler()
        var scanner = ToolCallScanner()
        let scanningForCalls = !params.tools.isEmpty
        var emittedToolCall = false
        var stopMatcher = StopMatcher(params.stop)
        var stoppedByMatch = false
        var splitter = ThinkSplitter()

        func emitText(_ text: String) {
            guard !text.isEmpty else { return }
            guard stopMatcher.isActive else {
                continuation.yield(.text(text))
                return
            }
            let safe = stopMatcher.feed(text)
            if !safe.isEmpty { continuation.yield(.text(safe)) }
            if stopMatcher.stopped { stoppedByMatch = true }
        }

        func emitRaw(_ text: String) {
            for piece in splitter.feed(text) {
                switch piece {
                case .text(let value): emitText(value)
                case .thinking(let value): continuation.yield(.thinking(value))
                }
            }
        }

        while generated < maxTokens {
            await Task.yield()
            if Task.isCancelled { break }
            var token = llama_sampler_sample(sampler, context, -1)
            if llama_vocab_is_eog(vocab, token) { break }

            let written = Self.renderPiece(into: &pieceBuffer) { buffer, capacity in
                llama_token_to_piece(vocab, token, buffer, capacity, 0, false)
            }
            if let written {
                let piece = assembler.feed(
                    pieceBuffer[0..<written].map { UInt8(bitPattern: $0) })
                if !piece.isEmpty {
                    if scanningForCalls {
                        let scanned = scanner.feed(piece)
                        if !scanned.text.isEmpty { emitRaw(scanned.text) }
                        if let call = scanned.call {
                            continuation.yield(.toolCall(call))
                            emittedToolCall = true
                        }
                    } else {
                        emitRaw(piece)
                    }
                }
            }
            generated += 1
            if emittedToolCall || stoppedByMatch { break }

            batch = llama_batch_get_one(&token, 1)
            guard llama_decode(context, batch) == 0 else {
                throw KernelError.runtimeFailed("llama decode failed mid-generation")
            }
        }

        if !stoppedByMatch {
            var remainder = assembler.flush()
            if scanningForCalls {
                let scanned = scanner.feed(remainder)
                remainder = scanned.text + scanner.flush()
                if let call = scanned.call {
                    continuation.yield(.toolCall(call))
                    emittedToolCall = true
                }
            }
            emitRaw(remainder)
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

        var finishReason: String?
        if emittedToolCall {
            finishReason = "tool_calls"
        } else if stoppedByMatch {
            finishReason = "stop"
        } else if generated >= maxTokens {
            finishReason = "length"
        }
        let elapsed = clock.now - started
        continuation.yield(
            .done(
                GenerationStats(
                    promptTokens: promptCount,
                    completionTokens: generated,
                    durationMs: Int(elapsed.components.seconds) * 1000
                        + Int(elapsed.components.attoseconds / 1_000_000_000_000_000),
                    loadMs: loadMs,
                    finishReason: finishReason)))
    }

    private func stampLoad(since start: ContinuousClock.Instant) {
        pendingLoadMs = Int((ContinuousClock.now - start) / .milliseconds(1))
    }

    static func messagesWithToolBlock(
        _ messages: [ChatMessage], tools: [ToolSpec]
    ) -> [ChatMessage] {
        guard !tools.isEmpty else { return messages }
        let block = ToolGrammar.systemBlock(for: tools)
        var extended = messages
        if let first = extended.first, first.role == .system {
            extended[0] = ChatMessage(
                role: .system, content: first.content + "\n\n" + block)
        } else {
            extended.insert(ChatMessage(role: .system, content: block), at: 0)
        }
        return extended
    }

    private func load(path: String, modelID: String, contextTokens: Int) throws {
        _ = Self.backendReady
        unload()

        let modelParams = llama_model_default_params()
        guard let loaded = llama_model_load_from_file(path, modelParams) else {
            throw KernelError.runtimeFailed("could not load gguf at \(path)")
        }
        var contextParams = llama_context_default_params()
        contextParams.n_ctx = UInt32(contextTokens)
        contextParams.n_batch = UInt32(contextTokens)
        guard let created = llama_init_from_model(loaded, contextParams) else {
            llama_model_free(loaded)
            throw KernelError.runtimeFailed("could not create llama context")
        }
        model = loaded
        context = created
        loadedPath = path
        loadedModelID = modelID
        loadedContextTokens = contextTokens
    }

    func unloadIfLoaded(path: String) {
        guard loadedPath == path else { return }
        unload()
    }

    static func renderPiece(
        into buffer: inout [CChar],
        write: (UnsafeMutablePointer<CChar>, Int32) -> Int32
    ) -> Int? {
        let written = buffer.withUnsafeMutableBufferPointer {
            write($0.baseAddress!, Int32($0.count))
        }
        if written > 0 { return Int(written) }
        guard written < 0 else { return nil }
        buffer = [CChar](repeating: 0, count: Int(-written))
        let retried = buffer.withUnsafeMutableBufferPointer {
            write($0.baseAddress!, Int32($0.count))
        }
        return retried > 0 ? Int(retried) : nil
    }

    private func unload() {
        if let context { llama_free(context) }
        if let model { llama_model_free(model) }
        context = nil
        model = nil
        loadedPath = nil
        loadedModelID = nil
        loadedContextTokens = nil
    }

    private func renderPrompt(
        model: OpaquePointer, messages: [ChatMessage]
    ) -> (prompt: String, fellBack: Bool) {
        let template = llama_model_chat_template(model, nil).map { String(cString: $0) }
        if let template, let rendered = Self.applyTemplate(template, messages: messages) {
            return (rendered, false)
        }
        return (ChatMLPrompt.render(messages), true)
    }

    static func applyTemplate(_ template: String, messages: [ChatMessage]) -> String? {
        var cMessages: [llama_chat_message] = []
        var allocations: [UnsafeMutablePointer<CChar>] = []
        defer { allocations.forEach { free($0) } }

        for message in messages {
            let role = strdup(message.role.rawValue)!
            let content = strdup(message.content)!
            allocations.append(role)
            allocations.append(content)
            cMessages.append(llama_chat_message(role: role, content: content))
        }

        var buffer = [CChar](repeating: 0, count: 16384)
        let needed = llama_chat_apply_template(
            template, &cMessages, cMessages.count, true, &buffer, Int32(buffer.count))
        guard needed > 0 else { return nil }
        if Int(needed) > buffer.count {
            buffer = [CChar](repeating: 0, count: Int(needed) + 1)
            let second = llama_chat_apply_template(
                template, &cMessages, cMessages.count, true, &buffer, Int32(buffer.count))
            guard second > 0 else { return nil }
            return String(
                decoding: buffer[0..<Int(second)].map { UInt8(bitPattern: $0) }, as: UTF8.self)
        }
        return String(
            decoding: buffer[0..<Int(needed)].map { UInt8(bitPattern: $0) }, as: UTF8.self)
    }

    static func chatMLPrompt(messages: [ChatMessage]) -> String {
        ChatMLPrompt.render(messages)
    }

    private func tokenize(vocab: OpaquePointer?, text: String) -> [llama_token] {
        let utf8Count = Int32(text.utf8.count)
        var tokens = [llama_token](repeating: 0, count: Int(utf8Count) + 16)
        let count = llama_tokenize(
            vocab, text, utf8Count, &tokens, Int32(tokens.count), true, true)
        guard count >= 0 else { return [] }
        return Array(tokens[0..<Int(count)])
    }
}
