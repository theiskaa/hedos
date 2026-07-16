import Foundation

public struct HardwareProfile: Sendable, Hashable {
    public let chip: String
    public let ramGB: Int
    public let totalMemoryBytes: UInt64

    public init(chip: String, ramGB: Int, totalMemoryBytes: UInt64? = nil) {
        self.chip = chip
        self.ramGB = ramGB
        self.totalMemoryBytes = totalMemoryBytes ?? UInt64(max(ramGB, 1)) << 30
    }

    public static let current = HardwareProfile(
        chip: readChip(),
        ramGB: max(1, Int(ProcessInfo.processInfo.physicalMemory / (1 << 30))),
        totalMemoryBytes: ProcessInfo.processInfo.physicalMemory)

    public var summary: String {
        "\(chip) · \(ramGB) GB unified"
    }

    private static func readChip() -> String {
        var size = 0
        sysctlbyname("machdep.cpu.brand_string", nil, &size, nil, 0)
        guard size > 0 else { return "Apple Silicon" }
        var buffer = [CChar](repeating: 0, count: size)
        sysctlbyname("machdep.cpu.brand_string", &buffer, &size, nil, 0)
        let value =
            buffer.withUnsafeBufferPointer { $0.baseAddress.map { String(cString: $0) } } ?? ""
        return value.isEmpty ? "Apple Silicon" : value
    }
}
