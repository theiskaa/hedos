import Foundation

public actor ResidencyManager {
    public typealias Unloader = @Sendable () async -> Bool

    private let defaultWarmWindow: Duration
    private var warmWindows: [String: Duration] = [:]
    private var unloaders: [String: Unloader] = [:]
    private var idleTasks: [String: Task<Void, Never>] = [:]
    private var inflightUnloads: [String: Task<Bool, Never>] = [:]

    public init(defaultWarmWindow: Duration = .seconds(120)) {
        self.defaultWarmWindow = defaultWarmWindow
    }

    public func register(
        _ modelID: String, warmWindow: Duration? = nil, unloader: @escaping Unloader
    ) {
        if let warmWindow {
            warmWindows[modelID] = warmWindow
        }
        unloaders[modelID] = unloader
        cancelIdleUnload(modelID)
    }

    public func setWarmWindow(_ window: Duration, for modelID: String) {
        warmWindows[modelID] = window
    }

    public func warmWindow(for modelID: String) -> Duration {
        warmWindows[modelID] ?? defaultWarmWindow
    }

    public func scheduleIdleUnload(_ modelID: String) {
        guard unloaders[modelID] != nil else { return }
        idleTasks[modelID]?.cancel()
        let window = warmWindow(for: modelID)
        idleTasks[modelID] = Task {
            try? await Task.sleep(for: window)
            guard !Task.isCancelled else { return }
            await self.unloadNow(modelID)
        }
    }

    public func cancelIdleUnload(_ modelID: String) {
        idleTasks[modelID]?.cancel()
        idleTasks[modelID] = nil
    }

    public func unloadNow(_ modelID: String) async {
        cancelIdleUnload(modelID)
        if let inflight = inflightUnloads[modelID] {
            _ = await inflight.value
            return
        }
        guard let unloader = unloaders.removeValue(forKey: modelID) else { return }
        let attempt = Task { await unloader() }
        inflightUnloads[modelID] = attempt
        let unloaded = await attempt.value
        inflightUnloads[modelID] = nil
        if !unloaded, unloaders[modelID] == nil {
            unloaders[modelID] = unloader
        }
    }

    public func suspendAll() {
        for task in idleTasks.values {
            task.cancel()
        }
        idleTasks = [:]
    }
}
