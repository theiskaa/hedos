/// The Hedos kernel — the stateful brain between the models on this machine
/// and everything that wants to use them: registry, resolution, execution,
/// and serving, behind one interface.
///
/// This is the M1 seed; subsystems arrive milestone by milestone.
public struct Kernel: Sendable {
    /// Kernel version, independent of the app's marketing version.
    public static let version = "0.0.1"

    public init() {}
}
