import Foundation
import LlamaSwift

public actor LlamaEngine {
    public static let shared = LlamaEngine()

    private static let backendReady: Void = {
        llama_backend_init()
    }()

    private var loadedPath: String?
    private var model: OpaquePointer?
    private var context: OpaquePointer?

    public func run(
        path: String,
        messages: [ChatMessage],
        continuation: AsyncThrowingStream<CapabilityChunk, Error>.Continuation
    ) async {
        do {
            try ensureLoaded(path: path)
            guard let model, let context else {
                throw KernelError.runtimeFailed("llama model not loaded")
            }
            let vocab = llama_model_get_vocab(model)
            let prompt = renderPrompt(model: model, messages: messages)
            var tokens = tokenize(vocab: vocab, text: prompt)
            guard !tokens.isEmpty else {
                throw KernelError.runtimeFailed("prompt produced no tokens")
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
            llama_sampler_chain_add(sampler, llama_sampler_init_temp(0.7))
            llama_sampler_chain_add(sampler, llama_sampler_init_dist(UInt32.random(in: 0..<UInt32.max)))
            defer { llama_sampler_free(sampler) }

            var generated = 0
            let maxTokens = 2048
            var pieceBuffer = [CChar](repeating: 0, count: 512)

            while generated < maxTokens {
                await Task.yield()
                if Task.isCancelled { break }
                var token = llama_sampler_sample(sampler, context, -1)
                if llama_vocab_is_eog(vocab, token) { break }

                let written = llama_token_to_piece(vocab, token, &pieceBuffer, 512, 0, false)
                if written > 0 {
                    let piece = String(
                        decoding: pieceBuffer[0..<Int(written)].map { UInt8(bitPattern: $0) },
                        as: UTF8.self)
                    if !piece.isEmpty {
                        continuation.yield(.text(piece))
                    }
                }
                generated += 1

                batch = llama_batch_get_one(&token, 1)
                guard llama_decode(context, batch) == 0 else {
                    throw KernelError.runtimeFailed("llama decode failed mid-generation")
                }
            }

            let elapsed = clock.now - started
            continuation.yield(
                .done(
                    GenerationStats(
                        promptTokens: promptCount,
                        completionTokens: generated,
                        durationMs: Int(elapsed.components.seconds) * 1000
                            + Int(elapsed.components.attoseconds / 1_000_000_000_000_000))))
            continuation.finish()
        } catch {
            continuation.finish(throwing: error)
        }
    }

    private func ensureLoaded(path: String) throws {
        _ = Self.backendReady
        if loadedPath == path, model != nil, context != nil { return }
        unload()

        let modelParams = llama_model_default_params()
        guard let loaded = llama_model_load_from_file(path, modelParams) else {
            throw KernelError.runtimeFailed("could not load gguf at \(path)")
        }
        var contextParams = llama_context_default_params()
        contextParams.n_ctx = 4096
        guard let created = llama_init_from_model(loaded, contextParams) else {
            llama_model_free(loaded)
            throw KernelError.runtimeFailed("could not create llama context")
        }
        model = loaded
        context = created
        loadedPath = path
    }

    public func shutdown() {
        unload()
    }

    private func unload() {
        if let context { llama_free(context) }
        if let model { llama_model_free(model) }
        context = nil
        model = nil
        loadedPath = nil
    }

    private func renderPrompt(model: OpaquePointer, messages: [ChatMessage]) -> String {
        let template = llama_model_chat_template(model, nil).map { String(cString: $0) }
        if let template, let rendered = Self.applyTemplate(template, messages: messages) {
            return rendered
        }
        return Self.chatMLPrompt(messages: messages)
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
        var prompt = ""
        for message in messages {
            prompt += "<|im_start|>\(message.role.rawValue)\n\(message.content)<|im_end|>\n"
        }
        prompt += "<|im_start|>assistant\n"
        return prompt
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
