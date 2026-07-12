import Foundation

public enum ConformanceClass: String, CaseIterable, Sendable {
    case ggufChat
    case ggufReasoning
    case mlxChat
    case ollamaChat
    case appleBuiltin
    case remoteEndpoint
    case diffusion
    case tts
    case whisper
    case vision
    case embedding
}

public enum ConformanceContract: String, CaseIterable, Sendable {
    case paramsHonored
    case templateCorrect
    case thinkingSeparated
    case overflowHonest
    case cancelClean
    case statsTruthful
    case toolsCallable
    case stateHonest
}

public struct ConformanceCell: Sendable, Hashable {
    public var model: String
    public var conformanceClass: ConformanceClass
    public var contract: ConformanceContract
    public var status: SweepStatus
    public var durationMs: Int
    public var reason: String?

    public init(
        model: String, conformanceClass: ConformanceClass, contract: ConformanceContract,
        status: SweepStatus, durationMs: Int, reason: String? = nil
    ) {
        self.model = model
        self.conformanceClass = conformanceClass
        self.contract = contract
        self.status = status
        self.durationMs = durationMs
        self.reason = reason
    }
}

public enum ConformanceMatrix {
    static let controlTokens = [
        "<|im_start|>", "<|im_end|>", "<|eot_id|>", "<|start_header_id|>",
        "<|end_header_id|>", "[INST]", "[/INST]", "<|assistant|>", "<|user|>",
    ]

    static let textClasses: Set<ConformanceClass> = [
        .ggufChat, .ggufReasoning, .mlxChat, .ollamaChat, .appleBuiltin, .remoteEndpoint,
        .vision,
    ]

    static func classify(_ record: ModelRecord) -> ConformanceClass? {
        if record.capabilities.contains(.embed) { return .embedding }
        if record.capabilities.contains(.see) { return .vision }
        if record.capabilities.contains(.transcribe) { return .whisper }
        if record.capabilities.contains(.speak) { return .tts }
        if record.capabilities.contains(.image) { return .diffusion }
        guard record.capabilities.contains(.chat) else { return nil }
        switch record.runtime.id {
        case .some(.openAIEndpoint): return .remoteEndpoint
        case .some(.appleFoundation): return .appleBuiltin
        case .some(.ollama): return .ollamaChat
        case .some(.mlxLm), .some(.mlxSwift): return .mlxChat
        case .some(.llamaCpp): return .ggufChat
        default:
            if record.source.kind == .endpoint { return .remoteEndpoint }
            if record.source.kind == .ollama { return .ollamaChat }
            return .ggufChat
        }
    }

    static func applies(_ contract: ConformanceContract, to conformanceClass: ConformanceClass)
        -> Bool
    {
        switch contract {
        case .stateHonest:
            return true
        case .statsTruthful:
            return textClasses.contains(conformanceClass) && conformanceClass != .remoteEndpoint
        case .cancelClean, .paramsHonored, .templateCorrect, .toolsCallable, .overflowHonest,
            .thinkingSeparated:
            return textClasses.contains(conformanceClass)
        }
    }

    public static func run(
        _ kernel: any ShelfSweepKernel,
        includeEndpoints: Bool = false,
        perCheckTimeout: Duration = .seconds(60)
    ) async -> [ConformanceCell] {
        let records: [ModelRecord]
        do {
            records = try await kernel.shelf()
        } catch {
            return []
        }
        var cells: [ConformanceCell] = []
        for record in records where record.state == .ready {
            if record.source.kind == .endpoint, !includeEndpoints { continue }
            guard var conformanceClass = classify(record) else { continue }

            let smoke = await smokeProbe(record, kernel: kernel, timeout: perCheckTimeout)
            if smoke.leakedThinking, conformanceClass == .ggufChat {
                conformanceClass = .ggufReasoning
            }
            for contract in ConformanceContract.allCases {
                guard applies(contract, to: conformanceClass) else {
                    cells.append(
                        ConformanceCell(
                            model: record.displayName, conformanceClass: conformanceClass,
                            contract: contract, status: .skip, durationMs: 0,
                            reason: "not applicable"))
                    continue
                }
                if let smokeReason = smoke.failure {
                    cells.append(
                        ConformanceCell(
                            model: record.displayName, conformanceClass: conformanceClass,
                            contract: contract, status: .fail, durationMs: 0, reason: smokeReason))
                    continue
                }
                let start = Date()
                let outcome = await checkWithTimeout(
                    contract, record: record, kernel: kernel, timeout: perCheckTimeout)
                cells.append(
                    ConformanceCell(
                        model: record.displayName, conformanceClass: conformanceClass,
                        contract: contract, status: outcome.status,
                        durationMs: elapsedMs(since: start), reason: outcome.reason))
            }
        }
        return cells
    }

    struct Smoke: Sendable {
        var failure: String?
        var leakedThinking: Bool
    }

    static func smokeProbe(
        _ record: ModelRecord, kernel: any ShelfSweepKernel, timeout: Duration
    ) async -> Smoke {
        guard textClasses.contains(classify(record) ?? .embedding) else {
            return Smoke(failure: nil, leakedThinking: false)
        }
        do {
            let stream = try await kernel.chat(
                record.id, messages: [ChatMessage(role: .user, content: "Say hi.")])
            var leaked = false
            for try await chunk in stream {
                if case .thinking = chunk { leaked = true }
                if case .text(let text) = chunk, text.contains("<think>") { leaked = true }
            }
            return Smoke(failure: nil, leakedThinking: leaked)
        } catch {
            return Smoke(
                failure: (error as? KernelError)?.errorDescription ?? String(describing: error),
                leakedThinking: false)
        }
    }

    struct Outcome: Sendable {
        var status: SweepStatus
        var reason: String?
    }

    struct CheckTimeout: Error {}

    static func checkWithTimeout(
        _ contract: ConformanceContract, record: ModelRecord, kernel: any ShelfSweepKernel,
        timeout: Duration
    ) async -> Outcome {
        do {
            return try await withThrowingTaskGroup(of: Outcome.self) { group in
                group.addTask { await check(contract, record: record, kernel: kernel) }
                group.addTask {
                    try await Task.sleep(for: timeout)
                    throw CheckTimeout()
                }
                let result = try await group.next()!
                group.cancelAll()
                return result
            }
        } catch is CheckTimeout {
            return Outcome(status: .fail, reason: "timed out")
        } catch {
            return Outcome(status: .fail, reason: String(describing: error))
        }
    }

    static func check(
        _ contract: ConformanceContract, record: ModelRecord, kernel: any ShelfSweepKernel
    ) async -> Outcome {
        switch contract {
        case .paramsHonored: return await checkParamsHonored(record, kernel: kernel)
        case .templateCorrect: return await checkTemplateCorrect(record, kernel: kernel)
        case .thinkingSeparated: return await checkThinkingSeparated(record, kernel: kernel)
        case .overflowHonest: return await checkOverflowHonest(record, kernel: kernel)
        case .cancelClean: return await checkCancelClean(record, kernel: kernel)
        case .statsTruthful: return await checkStatsTruthful(record, kernel: kernel)
        case .toolsCallable: return await checkToolsCallable(record, kernel: kernel)
        case .stateHonest: return await checkStateHonest(record, kernel: kernel)
        }
    }

    static func checkParamsHonored(_ record: ModelRecord, kernel: any ShelfSweepKernel) async
        -> Outcome
    {
        do {
            let stream = try await kernel.invoke(
                record.id, .chat,
                payload: .object([
                    "messages": .array([
                        ChatMessage(role: .user, content: "Count slowly.").payloadValue
                    ]),
                    "max_tokens": .int(8),
                ]))
            var completion: Int?
            for try await chunk in stream {
                if case .done(let stats) = chunk { completion = stats?.completionTokens }
            }
            guard let completion else {
                return Outcome(
                    status: .skip, reason: "runtime reported no token count to check the cap")
            }
            return completion <= 16
                ? Outcome(status: .pass, reason: nil)
                : Outcome(status: .fail, reason: "max_tokens 8 produced \(completion) tokens")
        } catch {
            return Outcome(status: .fail, reason: String(describing: error))
        }
    }

    static func checkTemplateCorrect(_ record: ModelRecord, kernel: any ShelfSweepKernel) async
        -> Outcome
    {
        do {
            var text = ""
            let stream = try await kernel.chat(
                record.id, messages: [ChatMessage(role: .user, content: "Say hello.")])
            for try await chunk in stream {
                if case .text(let delta) = chunk { text += delta }
            }
            if let leaked = controlTokens.first(where: text.contains) {
                return Outcome(status: .fail, reason: "raw control token \(leaked) in the reply")
            }
            return Outcome(status: .pass, reason: nil)
        } catch {
            return Outcome(status: .fail, reason: String(describing: error))
        }
    }

    static func checkThinkingSeparated(_ record: ModelRecord, kernel: any ShelfSweepKernel) async
        -> Outcome
    {
        do {
            let stream = try await kernel.chat(
                record.id, messages: [ChatMessage(role: .user, content: "Think, then answer 2+2.")])
            for try await chunk in stream {
                if case .text(let delta) = chunk, delta.contains("<think>") {
                    return Outcome(status: .fail, reason: "<think> leaked into visible text")
                }
            }
            return Outcome(status: .pass, reason: nil)
        } catch {
            return Outcome(status: .fail, reason: String(describing: error))
        }
    }

    static func checkOverflowHonest(_ record: ModelRecord, kernel: any ShelfSweepKernel) async
        -> Outcome
    {
        let huge = String(repeating: "context ", count: 200_000)
        do {
            let stream = try await kernel.chat(
                record.id, messages: [ChatMessage(role: .user, content: huge)])
            for try await _ in stream {}
            return Outcome(status: .pass, reason: "trimmed and answered")
        } catch let error as KernelError {
            if case .contextExceeded = error { return Outcome(status: .pass, reason: nil) }
            return Outcome(
                status: .fail, reason: "overflow surfaced as \(error.errorDescription ?? "?")")
        } catch {
            return Outcome(status: .fail, reason: String(describing: error))
        }
    }

    static func checkCancelClean(_ record: ModelRecord, kernel: any ShelfSweepKernel) async
        -> Outcome
    {
        func pullFirstChunkThenTearDown() async throws {
            let stream = try await kernel.chat(
                record.id, messages: [ChatMessage(role: .user, content: "Tell a long story.")])
            var iterator = stream.makeAsyncIterator()
            _ = try await iterator.next()
        }
        do {
            try await pullFirstChunkThenTearDown()
            let followup = try await kernel.chat(
                record.id, messages: [ChatMessage(role: .user, content: "Just say ok.")])
            var produced = false
            for try await chunk in followup {
                if case .text = chunk { produced = true }
                if case .done = chunk { break }
            }
            return produced
                ? Outcome(status: .pass, reason: nil)
                : Outcome(status: .fail, reason: "the follow-up request produced nothing")
        } catch {
            return Outcome(status: .fail, reason: "cancel wedged the record: \(error)")
        }
    }

    static func checkStatsTruthful(_ record: ModelRecord, kernel: any ShelfSweepKernel) async
        -> Outcome
    {
        do {
            let stream = try await kernel.chat(
                record.id, messages: [ChatMessage(role: .user, content: "Hi.")])
            var stats: GenerationStats?
            for try await chunk in stream {
                if case .done(let done) = chunk { stats = done }
            }
            guard let stats else { return Outcome(status: .fail, reason: "no .done stats") }
            guard (stats.completionTokens ?? 0) > 0, (stats.durationMs ?? 0) > 0 else {
                return Outcome(status: .fail, reason: "stats present but zero")
            }
            return Outcome(status: .pass, reason: nil)
        } catch {
            return Outcome(status: .fail, reason: String(describing: error))
        }
    }

    static func checkToolsCallable(_ record: ModelRecord, kernel: any ShelfSweepKernel) async
        -> Outcome
    {
        let tool = ToolSpec(
            name: "get_time",
            description: "Returns the current time.",
            parameters: .object(["type": .string("object"), "properties": .object([:])]))
        do {
            let stream = try await kernel.invoke(
                record.id, .chat,
                payload: .object([
                    "messages": .array([
                        ChatMessage(role: .user, content: "What time is it? Use the tool.")
                            .payloadValue
                    ]),
                    "tools": .array([tool.payloadValue]),
                ]))
            for try await chunk in stream {
                if case .toolCall = chunk { return Outcome(status: .pass, reason: nil) }
            }
            return Outcome(status: .fail, reason: "no tool-call chunk emitted")
        } catch {
            return Outcome(status: .fail, reason: String(describing: error))
        }
    }

    static func checkStateHonest(_ record: ModelRecord, kernel: any ShelfSweepKernel) async
        -> Outcome
    {
        let capability = primaryCapability(record)
        if record.execution == .job {
            do {
                let jobID = try await kernel.submit(
                    record.id, capability, payload: probePayload(for: capability))
                await kernel.cancel(jobID: jobID)
                return Outcome(status: .pass, reason: nil)
            } catch KernelError.capabilityUnsupported {
                return Outcome(
                    status: .fail, reason: "ready record cannot serve \(capability.rawValue)")
            } catch KernelError.runtimeUnavailable(let hint) {
                return Outcome(status: .fail, reason: "ready record has no runtime: \(hint)")
            } catch {
                return Outcome(status: .pass, reason: nil)
            }
        }
        do {
            let stream = try await kernel.invoke(
                record.id, capability, payload: probePayload(for: capability))
            for try await _ in stream {}
            return Outcome(status: .pass, reason: nil)
        } catch KernelError.capabilityUnsupported {
            return Outcome(
                status: .fail, reason: "ready record cannot serve \(capability.rawValue)")
        } catch KernelError.runtimeUnavailable(let hint) {
            return Outcome(status: .fail, reason: "ready record has no runtime: \(hint)")
        } catch let error as KernelError {
            switch error {
            case .runtimeFailed, .sidecarDied, .bundleMissing, .wrongExecutionMode:
                return Outcome(
                    status: .fail,
                    reason: "ready record's runtime failed: \(error.errorDescription ?? "")")
            default:
                return Outcome(status: .pass, reason: nil)
            }
        } catch {
            return Outcome(status: .pass, reason: nil)
        }
    }

    static func primaryCapability(_ record: ModelRecord) -> Capability {
        for capability in [Capability.chat, .embed, .image, .speak, .transcribe, .see] {
            if record.capabilities.contains(capability) { return capability }
        }
        return .chat
    }

    static func probePayload(for capability: Capability) -> JSONValue {
        switch capability {
        case .embed: return .object(["input": .string("hedos")])
        case .image, .speak: return .object(["prompt": .string("hedos")])
        case .transcribe:
            return .object(["audio": .string(ShelfSweep.transcribeFixtureURL()?.path ?? "")])
        case .see:
            let message = ChatMessage(
                role: .user, content: "hi",
                attachments: (ShelfSweep.seeFixtureURL().flatMap { try? Data(contentsOf: $0) })
                    .map { [ChatAttachment(kind: .image, data: $0, mimeType: "image/png")] } ?? [])
            return .object(["messages": .array([message.payloadValue])])
        default:
            return .object([
                "messages": .array([ChatMessage(role: .user, content: "hi").payloadValue])
            ])
        }
    }

    static func elapsedMs(since start: Date) -> Int {
        max(0, Int(Date().timeIntervalSince(start) * 1000))
    }
}

public struct ConformanceBaseline: Codable, Sendable {
    public struct Key: Codable, Sendable, Hashable {
        public var model: String
        public var conformanceClass: String
        public var contract: String
    }

    public var passing: [Key]

    public init(passing: [Key]) { self.passing = passing }

    static func key(_ cell: ConformanceCell) -> Key {
        Key(
            model: cell.model, conformanceClass: cell.conformanceClass.rawValue,
            contract: cell.contract.rawValue)
    }

    public static func from(_ cells: [ConformanceCell]) -> ConformanceBaseline {
        ConformanceBaseline(passing: cells.filter { $0.status == .pass }.map(key))
    }

    public func regressions(in cells: [ConformanceCell]) -> [ConformanceCell] {
        let byKey = Dictionary(cells.map { (Self.key($0), $0) }, uniquingKeysWith: { first, _ in
            first
        })
        return passing.compactMap { key in
            let current = byKey[key]
            if current?.status == .pass { return nil }
            return current
                ?? ConformanceCell(
                    model: key.model,
                    conformanceClass: ConformanceClass(rawValue: key.conformanceClass) ?? .ggufChat,
                    contract: ConformanceContract(rawValue: key.contract) ?? .stateHonest,
                    status: .skip, durationMs: 0, reason: "no longer present on the shelf")
        }
    }

    static func url(kernelDirectory: URL) -> URL {
        kernelDirectory.appendingPathComponent("conformance-baseline.json")
    }

    public static func load(kernelDirectory: URL) -> ConformanceBaseline? {
        guard let data = try? Data(contentsOf: url(kernelDirectory: kernelDirectory)) else {
            return nil
        }
        return try? JSONDecoder().decode(ConformanceBaseline.self, from: data)
    }

    public func save(kernelDirectory: URL) throws {
        try JSONEncoder().encode(self)
            .write(to: Self.url(kernelDirectory: kernelDirectory), options: .atomic)
    }
}

public enum EnvironmentalGaps {
    public static func open(shelf: [ModelRecord]) -> [String] {
        var gaps: [String] = []
        let servesTranscription = shelf.contains { record in
            record.state == .ready && record.capabilities.contains(.transcribe)
                && record.runtime.id != nil
        }
        if !servesTranscription {
            gaps.append(
                "no whisper ggml binary present — the transcription path is fake-sidecar-tested "
                + "only until one is fetched (report-and-wait, theiskaa's to supply)")
        }
        gaps.append(
            "the pre-baked kokoro VM image is not yet available — the community kokoro install "
            + "gate stays blocked until it is baked (report-and-wait, theiskaa's to supply)")
        return gaps
    }
}

public enum ConformanceReport {
    public static func render(_ cells: [ConformanceCell]) -> String {
        let lines = cells.map(cellLine)
        return (lines + [""] + rollup(cells)).joined(separator: "\n")
    }

    static func cellLine(_ cell: ConformanceCell) -> String {
        [
            cell.model,
            cell.conformanceClass.rawValue,
            cell.contract.rawValue,
            cell.status.rawValue,
            "\(cell.durationMs)ms",
            cell.reason ?? "-",
        ].joined(separator: " · ")
    }

    static func rollup(_ cells: [ConformanceCell]) -> [String] {
        ConformanceClass.allCases.compactMap { conformanceClass in
            let classCells = cells.filter { $0.conformanceClass == conformanceClass }
            guard !classCells.isEmpty else { return nil }
            let pass = classCells.filter { $0.status == .pass }.count
            let fail = classCells.filter { $0.status == .fail }.count
            let skip = classCells.filter { $0.status == .skip }.count
            return "\(conformanceClass.rawValue): \(pass) pass · \(fail) fail · \(skip) skip"
        }
    }
}
