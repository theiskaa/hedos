extension Kernel {
    func applyStoredPolicies() async {
        await governor.apply(policy: settings.models().residencyPolicy)
        await scheduler.history.setLimit(settings.advanced().jobHistoryLimit)
    }
}
