import Darwin
import Foundation
import SwiftUI

struct SystemMemory: Sendable {
    let totalBytes: UInt64
    let usedBytes: UInt64
    let freeBytes: UInt64

    var usedFraction: Double {
        totalBytes == 0 ? 0 : min(Double(usedBytes) / Double(totalBytes), 1)
    }

    static func sample() -> SystemMemory? {
        var stats = vm_statistics64_data_t()
        var count = mach_msg_type_number_t(
            MemoryLayout<vm_statistics64_data_t>.stride / MemoryLayout<integer_t>.stride)
        let host = mach_host_self()
        let result = withUnsafeMutablePointer(to: &stats) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                host_statistics64(host, HOST_VM_INFO64, $0, &count)
            }
        }
        guard result == KERN_SUCCESS else { return nil }

        var pageSize: vm_size_t = 0
        host_page_size(host, &pageSize)
        let page = UInt64(pageSize == 0 ? 16384 : pageSize)
        let total = ProcessInfo.processInfo.physicalMemory
        let wired = UInt64(stats.wire_count) * page
        let compressed = UInt64(stats.compressor_page_count) * page
        let purgeable = UInt64(stats.purgeable_count) * page
        let internalPages = UInt64(stats.internal_page_count) * page
        let free = UInt64(stats.free_count) * page
        let app = internalPages >= purgeable ? internalPages - purgeable : 0
        let used = min(app + wired + compressed, total)
        return SystemMemory(
            totalBytes: total, usedBytes: used, freeBytes: total > used ? total - used : free)
    }
}

struct SystemSnapshot: Sendable {
    var memory: SystemMemory?
    var temperature: Double?
}

actor SystemSampler {
    private typealias CopyEvent =
        @convention(c) (CFTypeRef, Int64, Int32, Int64) -> Unmanaged<CFTypeRef>?
    private typealias FloatValue = @convention(c) (CFTypeRef, Int32) -> Double

    private var client: CFTypeRef?
    private var services: [AnyObject] = []
    private var copyEvent: CopyEvent?
    private var floatValue: FloatValue?
    private var prepared = false

    func snapshot() -> SystemSnapshot {
        SystemSnapshot(memory: SystemMemory.sample(), temperature: temperature())
    }

    private func temperature() -> Double? {
        prepare()
        guard let copyEvent, let floatValue, !services.isEmpty else { return nil }
        let temperatureType: Int64 = 15
        let field = Int32(temperatureType << 16)
        var readings: [Double] = []
        for service in services {
            guard let event = copyEvent(service as CFTypeRef, temperatureType, 0, 0)?
                .takeRetainedValue()
            else { continue }
            let value = floatValue(event, field)
            if value > 0, value < 150 { readings.append(value) }
        }
        guard !readings.isEmpty else { return nil }
        return readings.reduce(0, +) / Double(readings.count)
    }

    private static func symbol<T>(_ name: String, _ type: T.Type) -> T? {
        guard let pointer = dlsym(UnsafeMutableRawPointer(bitPattern: -2), name) else { return nil }
        return unsafeBitCast(pointer, to: T.self)
    }

    private func prepare() {
        guard !prepared else { return }
        prepared = true

        typealias Create = @convention(c) (CFAllocator?) -> Unmanaged<CFTypeRef>?
        typealias SetMatching = @convention(c) (CFTypeRef, CFDictionary) -> Void
        typealias CopyServices = @convention(c) (CFTypeRef) -> Unmanaged<CFArray>?
        typealias CopyProperty = @convention(c) (CFTypeRef, CFString) -> Unmanaged<CFTypeRef>?

        guard
            let create = Self.symbol("IOHIDEventSystemClientCreate", Create.self),
            let setMatching = Self.symbol("IOHIDEventSystemClientSetMatching", SetMatching.self),
            let copyServices = Self.symbol("IOHIDEventSystemClientCopyServices", CopyServices.self),
            let copyProperty = Self.symbol("IOHIDServiceClientCopyProperty", CopyProperty.self),
            let copyEventFn = Self.symbol("IOHIDServiceClientCopyEvent", CopyEvent.self),
            let floatFn = Self.symbol("IOHIDEventGetFloatValue", FloatValue.self),
            let created = create(kCFAllocatorDefault)?.takeRetainedValue()
        else { return }

        setMatching(created, ["PrimaryUsagePage": 0xff00, "PrimaryUsage": 5] as CFDictionary)
        guard let all = copyServices(created)?.takeRetainedValue() as? [AnyObject] else { return }

        services = all.filter { service in
            guard
                let name = copyProperty(service as CFTypeRef, "Product" as CFString)?
                    .takeRetainedValue() as? String
            else { return false }
            return name.contains("MTR Temp") || name.hasPrefix("PMU tdie")
        }
        copyEvent = copyEventFn
        floatValue = floatFn
        client = created
    }
}

@MainActor
@Observable
final class SystemMonitor {
    var memory: SystemMemory?
    var temperatureC: Double?
    var thermal: ProcessInfo.ThermalState = ProcessInfo.processInfo.thermalState

    @ObservationIgnored private var loop: Task<Void, Never>?
    @ObservationIgnored private let sampler = SystemSampler()

    var thermalLabel: String {
        switch thermal {
        case .nominal: "Normal"
        case .fair: "Warm"
        case .serious: "Hot"
        case .critical: "Throttling"
        @unknown default: "—"
        }
    }

    var runningHot: Bool {
        if let temperatureC { return temperatureC >= 80 }
        return thermal == .serious || thermal == .critical
    }

    func start() {
        guard loop == nil else { return }
        loop = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                let snapshot = await self.sampler.snapshot()
                self.memory = snapshot.memory
                self.temperatureC = snapshot.temperature
                self.thermal = ProcessInfo.processInfo.thermalState
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    func stop() {
        loop?.cancel()
        loop = nil
    }
}
