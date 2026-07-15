import Foundation

public protocol ShelfSweepKernel: Sendable {
    func shelf() async throws -> [ModelRecord]
    func chat(_ modelID: String, messages: [ChatMessage]) async throws -> AsyncThrowingStream<
        CapabilityChunk, Error
    >
    func invoke(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> AsyncThrowingStream<CapabilityChunk, Error>
    func submit(
        _ modelID: String, _ capability: Capability, payload: JSONValue
    ) async throws -> String
    func job(id: String) async throws -> Job?
    func jobEvents(id: String) async -> AsyncStream<JobEvent>
    func cancel(jobID: String) async
}

extension Kernel: ShelfSweepKernel {}

public enum ShelfSweep {
    static let priority: [Capability] = [.chat, .see, .embed, .image, .speak, .transcribe]

    public static func run(
        _ kernel: any ShelfSweepKernel,
        includeEndpoints: Bool = false,
        includeImage: Bool = false,
        transcribeFixture: URL? = transcribeFixtureURL(),
        seeFixture: URL? = seeFixtureURL(),
        perModelTimeout: Duration = .seconds(120)
    ) async -> [SweepResult] {
        let records: [ModelRecord]
        do {
            records = try await kernel.shelf()
        } catch {
            return [
                SweepResult(
                    model: "shelf", capability: nil, status: .fail, durationMs: 0,
                    reason: "registry unreadable: \(error.localizedDescription)")
            ]
        }
        var results: [SweepResult] = []
        for record in records {
            if record.state != .ready {
                results.append(skipped(record, capability: nil, reason: "not ready"))
                continue
            }
            if record.source.kind == .endpoint, !includeEndpoints {
                results.append(skipped(record, capability: nil, reason: "endpoint runtime"))
                continue
            }
            let capabilities = priority.filter { record.capabilities.contains($0) }
            guard !capabilities.isEmpty else {
                results.append(
                    skipped(record, capability: nil, reason: "no sweepable capability"))
                continue
            }
            for capability in capabilities {
                results.append(
                    await sweepWithTimeout(
                        record, capability: capability, kernel: kernel,
                        includeImage: includeImage, transcribeFixture: transcribeFixture,
                        seeFixture: seeFixture, timeout: perModelTimeout))
            }
        }
        return results
    }

    struct SweepTimeout: Error {}

    static func sweepWithTimeout(
        _ record: ModelRecord,
        capability: Capability,
        kernel: any ShelfSweepKernel,
        includeImage: Bool,
        transcribeFixture: URL?,
        seeFixture: URL?,
        timeout: Duration
    ) async -> SweepResult {
        let start = Date()
        do {
            return try await withThrowingTaskGroup(of: SweepResult.self) { group in
                group.addTask {
                    await sweep(
                        record, capability: capability, kernel: kernel,
                        includeImage: includeImage, transcribeFixture: transcribeFixture,
                        seeFixture: seeFixture)
                }
                group.addTask {
                    try await Task.sleep(for: timeout)
                    throw SweepTimeout()
                }
                let result = try await group.next()!
                group.cancelAll()
                return result
            }
        } catch is SweepTimeout {
            return SweepResult(
                model: record.displayName, capability: capability, status: .fail,
                durationMs: elapsedMs(since: start),
                reason: "timed out after \(timeoutSeconds(timeout))s")
        } catch {
            return SweepResult(
                model: record.displayName, capability: nil, status: .fail,
                durationMs: elapsedMs(since: start),
                reason: String(describing: error))
        }
    }

    static func timeoutSeconds(_ duration: Duration) -> Int {
        Int(duration.components.seconds)
    }

    static func sweep(
        _ record: ModelRecord,
        capability: Capability,
        kernel: any ShelfSweepKernel,
        includeImage: Bool,
        transcribeFixture: URL?,
        seeFixture: URL?
    ) async -> SweepResult {
        if capability == .image, !includeImage {
            return skipped(
                record, capability: capability,
                reason: "image generation not run in sweep — pass --include-image")
        }

        let start = Date()
        let parity: SweepParity?
        do {
            parity = try await dispatch(
                capability, record: record, kernel: kernel,
                transcribeFixture: transcribeFixture, seeFixture: seeFixture)
        } catch {
            if let kernelError = error as? KernelError, isOllamaDaemonDown(record, kernelError) {
                return skipped(
                    record, capability: capability,
                    reason: kernelError.errorDescription ?? "ollama daemon unreachable")
            }
            return SweepResult(
                model: record.displayName, capability: capability, status: .fail,
                durationMs: elapsedMs(since: start),
                reason: (error as? KernelError)?.errorDescription ?? String(describing: error))
        }
        return SweepResult(
            model: record.displayName, capability: capability, status: .pass,
            durationMs: elapsedMs(since: start), parity: parity)
    }

    @discardableResult
    static func dispatch(
        _ capability: Capability,
        record: ModelRecord,
        kernel: any ShelfSweepKernel,
        transcribeFixture: URL?,
        seeFixture: URL?
    ) async throws -> SweepParity? {
        switch capability {
        case .chat:
            return try await sweepChat(record, kernel: kernel)
        case .see:
            guard let seeFixture, let imageData = try? Data(contentsOf: seeFixture) else {
                throw KernelError.runtimeFailed("no see fixture available")
            }
            let message = ChatMessage(
                role: .user, content: "What is in this image?",
                attachments: [
                    ChatAttachment(kind: .image, data: imageData, mimeType: "image/png")
                ])
            let stream = try await kernel.invoke(
                record.id, .chat, payload: .object(["messages": .array([message.payloadValue])]))
            try await drain(stream)
            return nil
        case .embed:
            let stream = try await kernel.invoke(
                record.id, .embed, payload: .object(["input": .string("hedos sweep")]))
            guard try await drainRequiringVector(stream) else {
                throw KernelError.runtimeFailed("embedding returned no vector")
            }
            return nil
        case .speak:
            let stream = try await kernel.invoke(
                record.id, .speak, payload: .object(["text": .string("Hedos shelf sweep check.")]))
            try await drain(stream)
            return nil
        case .transcribe:
            guard let transcribeFixture else {
                throw KernelError.runtimeFailed("no transcribe fixture available")
            }
            let stream = try await kernel.invoke(
                record.id, .transcribe, payload: .object(["audio": .string(transcribeFixture.path)]))
            try await drain(stream)
            return nil
        case .image:
            try await runImageJob(record, kernel: kernel)
            return nil
        default:
            throw KernelError.notImplemented("sweeping \(capability.rawValue)")
        }
    }

    static func sweepChat(
        _ record: ModelRecord, kernel: any ShelfSweepKernel
    ) async throws -> SweepParity {
        let stream = try await kernel.chat(
            record.id, messages: [ChatMessage(role: .user, content: "Say hi in three words.")])
        var thinkingSeparated = true
        var noticeFired = false
        var statsReported = false
        for try await chunk in stream {
            switch chunk {
            case .text(let text):
                if ThinkSplitter.hasVisibleTags(in: text) {
                    thinkingSeparated = false
                }
            case .status(let status):
                if status == ChatMLPrompt.noTemplateNotice { noticeFired = true }
            case .done:
                statsReported = true
            default:
                break
            }
        }

        var promptCompleteOK = true
        if record.capabilities.contains(.complete) {
            do {
                let completion = try await kernel.invoke(
                    record.id, .complete, payload: .object(["prompt": .string("Two plus two is")]))
                var produced = false
                for try await chunk in completion {
                    if case .text = chunk { produced = true }
                }
                promptCompleteOK = produced
            } catch {
                promptCompleteOK = false
            }
        }

        return SweepParity(
            thinkingSeparated: thinkingSeparated, templateNoticeFired: noticeFired,
            promptCompleteOK: promptCompleteOK, statsReported: statsReported)
    }

    static func drain(_ stream: AsyncThrowingStream<CapabilityChunk, Error>) async throws {
        for try await _ in stream {}
    }

    static func drainRequiringVector(
        _ stream: AsyncThrowingStream<CapabilityChunk, Error>
    ) async throws -> Bool {
        var sawVector = false
        for try await chunk in stream {
            if case .vector(let values) = chunk, !values.isEmpty { sawVector = true }
        }
        return sawVector
    }

    static func runImageJob(_ record: ModelRecord, kernel: any ShelfSweepKernel) async throws {
        let jobID = try await kernel.submit(
            record.id, .image, payload: .object(["prompt": .string("hedos shelf sweep check")]))
        let events = await kernel.jobEvents(id: jobID)
        try await withTaskCancellationHandler {
            for await event in events {
                switch event {
                case .failed(let message):
                    throw KernelError.runtimeFailed(message)
                case .cancelled:
                    throw KernelError.runtimeFailed("job was cancelled")
                default:
                    continue
                }
            }
            if let job = try await kernel.job(id: jobID), job.state == .failed {
                throw KernelError.runtimeFailed(job.error ?? "job failed")
            }
        } onCancel: {
            Task { await kernel.cancel(jobID: jobID) }
        }
    }

    static func isOllamaDaemonDown(_ record: ModelRecord, _ error: KernelError) -> Bool {
        guard record.runtime.id == .ollama else { return false }
        if case .runtimeUnavailable = error { return true }
        return false
    }

    static func skipped(_ record: ModelRecord, capability: Capability?, reason: String)
        -> SweepResult
    {
        SweepResult(
            model: record.displayName, capability: capability, status: .skip, durationMs: 0,
            reason: reason)
    }

    static func elapsedMs(since start: Date) -> Int {
        max(0, Int(Date().timeIntervalSince(start) * 1000))
    }

    public static func transcribeFixtureURL() -> URL? {
        fixtureURL(named: "sweep-probe.wav")
    }

    public static func seeFixtureURL() -> URL? {
        fixtureURL(named: "sweep-probe.png")
    }

    static func fixtureURL(named name: String) -> URL? {
        guard let root = Bundle.kernelModule.resourceURL else { return nil }
        let candidates = [
            root.appendingPathComponent("Resources/Fixtures/\(name)"),
            root.appendingPathComponent("Fixtures/\(name)"),
            root.deletingLastPathComponent()
                .appendingPathComponent("Resources/Fixtures/\(name)"),
        ]
        return candidates.first { FileManager.default.fileExists(atPath: $0.path) }
    }
}
