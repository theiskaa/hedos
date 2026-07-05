import Foundation

public actor JobScheduler {
    public typealias Runner = @Sendable () -> AsyncThrowingStream<JobRuntimeEvent, Error>

    public let history: JobHistoryStore

    private let admission: any JobAdmission
    private var jobs: [String: Job] = [:]
    private var queue: [String] = []
    private var runners: [String: Runner] = [:]
    private var subscribers: [String: [UUID: AsyncStream<JobEvent>.Continuation]] = [:]
    private var executing: (jobID: String, task: Task<Void, Never>)?
    private var cancelRequested: Set<String> = []

    public init(history: JobHistoryStore, admission: any JobAdmission = ImmediateAdmission()) {
        self.history = history
        self.admission = admission
    }

    public func submit(
        modelID: String,
        capability: Capability,
        payload: JSONValue,
        runner: @escaping Runner
    ) -> String {
        let job = Job(modelID: modelID, capability: capability, payload: payload)
        jobs[job.id] = job
        runners[job.id] = runner
        queue.append(job.id)
        startNextIfIdle()
        return job.id
    }

    public func job(id: String) async throws -> Job? {
        if let live = jobs[id] { return live }
        return try await history.get(id: id)
    }

    public func active() -> [Job] {
        jobs.values
            .filter { !$0.state.isTerminal }
            .sorted { ($0.submittedAt, $0.id) < ($1.submittedAt, $1.id) }
    }

    public func events(id: String) -> AsyncStream<JobEvent> {
        AsyncStream { continuation in
            guard let job = jobs[id] else {
                Task { [weak self] in
                    await self?.replayFromHistory(id, into: continuation)
                    continuation.finish()
                }
                return
            }
            for event in replay(job) {
                continuation.yield(event)
            }
            guard !job.state.isTerminal else {
                continuation.finish()
                return
            }
            let token = UUID()
            subscribers[id, default: [:]][token] = continuation
            continuation.onTermination = { _ in
                Task { [weak self] in await self?.dropSubscriber(id, token: token) }
            }
        }
    }

    public func cancel(_ jobID: String) async {
        guard let job = jobs[jobID], !job.state.isTerminal else { return }
        if let executing, executing.jobID == jobID {
            cancelRequested.insert(jobID)
            executing.task.cancel()
        } else {
            queue.removeAll { $0 == jobID }
            await conclude(jobID, as: .cancelled)
        }
    }

    private func startNextIfIdle() {
        guard executing == nil, !queue.isEmpty else { return }
        let jobID = queue.removeFirst()
        let task = Task { await self.execute(jobID) }
        executing = (jobID, task)
    }

    private func execute(_ jobID: String) async {
        guard let job = jobs[jobID], let runner = runners[jobID] else {
            await conclude(jobID, as: .failed, error: "job \(jobID) lost its runner")
            return
        }
        do {
            try await admission.admit(job) { [weak self] reason in
                await self?.markWaiting(jobID, reason: reason)
            }
        } catch is CancellationError {
            await conclude(jobID, as: .cancelled)
            return
        } catch {
            await conclude(jobID, as: .failed, error: error.localizedDescription)
            return
        }
        guard !cancelRequested.contains(jobID), !Task.isCancelled else {
            await conclude(jobID, as: .cancelled)
            return
        }
        mutate(jobID) {
            $0.state = .preparing
            $0.queueReason = nil
            $0.startedAt = Date()
        }
        emit(jobID, .preparing)
        do {
            for try await event in runner() {
                if Task.isCancelled { break }
                apply(event, to: jobID)
            }
            if cancelRequested.contains(jobID) || Task.isCancelled {
                await conclude(jobID, as: .cancelled)
            } else {
                await conclude(jobID, as: .done)
            }
        } catch is CancellationError {
            await conclude(jobID, as: .cancelled)
        } catch {
            await conclude(jobID, as: .failed, error: error.localizedDescription)
        }
    }

    private func apply(_ event: JobRuntimeEvent, to jobID: String) {
        switch event {
        case .status(let message):
            emit(jobID, .status(message))
        case .started:
            markRunning(jobID)
        case .progress(let step, let totalSteps):
            markRunning(jobID)
            guard let job = jobs[jobID] else { return }
            let fraction =
                totalSteps > 0 ? min(max(Double(step) / Double(totalSteps), 0), 1) : 0
            guard fraction >= job.progress.fraction else { return }
            let progress = JobProgress(fraction: fraction, step: step, totalSteps: totalSteps)
            mutate(jobID) { $0.progress = progress }
            emit(jobID, .progress(progress))
        case .preview(let frame):
            mutate(jobID) { $0.preview = frame }
            emit(jobID, .preview(frame))
        case .artifacts(let ids):
            mutate(jobID) { $0.result.append(contentsOf: ids) }
        }
    }

    private func markRunning(_ jobID: String) {
        guard let job = jobs[jobID], job.state != .running else { return }
        mutate(jobID) { $0.state = .running }
        emit(jobID, .running)
    }

    private func markWaiting(_ jobID: String, reason: String) {
        guard jobs[jobID]?.state == .queued else { return }
        mutate(jobID) { $0.queueReason = reason }
        emit(jobID, .queued(reason: reason))
    }

    private func conclude(_ jobID: String, as state: JobState, error: String? = nil) async {
        if var job = jobs[jobID], !job.state.isTerminal {
            job.state = state
            job.error = error
            job.finishedAt = Date()
            if state == .done {
                job.progress = JobProgress(
                    fraction: 1,
                    step: job.progress.totalSteps ?? job.progress.step,
                    totalSteps: job.progress.totalSteps)
            }
            jobs[jobID] = job
            try? await history.record(job)
            switch state {
            case .done:
                emit(jobID, .done(result: job.result))
            case .failed:
                emit(jobID, .failed(message: error ?? "failed"))
            case .cancelled:
                emit(jobID, .cancelled)
            default:
                break
            }
        }
        finishSubscribers(jobID)
        jobs[jobID] = nil
        runners[jobID] = nil
        cancelRequested.remove(jobID)
        if executing?.jobID == jobID {
            executing = nil
        }
        startNextIfIdle()
    }

    private func mutate(_ jobID: String, _ change: (inout Job) -> Void) {
        guard var job = jobs[jobID] else { return }
        change(&job)
        jobs[jobID] = job
    }

    private func emit(_ jobID: String, _ event: JobEvent) {
        guard let continuations = subscribers[jobID] else { return }
        for continuation in continuations.values {
            continuation.yield(event)
        }
    }

    private func finishSubscribers(_ jobID: String) {
        guard let continuations = subscribers.removeValue(forKey: jobID) else { return }
        for continuation in continuations.values {
            continuation.finish()
        }
    }

    private func dropSubscriber(_ jobID: String, token: UUID) {
        subscribers[jobID]?[token] = nil
    }

    private func replayFromHistory(
        _ jobID: String, into continuation: AsyncStream<JobEvent>.Continuation
    ) async {
        guard let job = try? await history.get(id: jobID), job.state.isTerminal else { return }
        for event in replay(job) {
            continuation.yield(event)
        }
    }

    private func replay(_ job: Job) -> [JobEvent] {
        var events: [JobEvent] = []
        switch job.state {
        case .queued:
            events.append(.queued(reason: job.queueReason))
        case .preparing:
            events.append(.preparing)
        case .running:
            events.append(.running)
            if job.progress.fraction > 0 {
                events.append(.progress(job.progress))
            }
        case .done:
            events.append(.done(result: job.result))
        case .failed:
            events.append(.failed(message: job.error ?? "failed"))
        case .cancelled:
            events.append(.cancelled)
        }
        if let preview = job.preview, !job.state.isTerminal {
            events.append(.preview(preview))
        }
        return events
    }
}
