import Foundation
import Testing

@testable import HedosKernel

private let machineMemory = UInt64(1000) << 20

@Test func fitVerdictSpansThresholdBoundaries() {
    func verdict(_ footprintMB: Int) -> FitVerdict? {
        FitVerdict.assess(footprintMB: footprintMB, totalMemoryBytes: machineMemory)?.verdict
    }

    #expect(verdict(100) == .runsWell)
    #expect(verdict(599) == .runsWell)
    #expect(verdict(600) == .tightFit)
    #expect(verdict(759) == .tightFit)
    #expect(verdict(760) == .tooLarge)
    #expect(verdict(2000) == .tooLarge)
}

@Test func fitVerdictExposesRequiredBytesForCopy() throws {
    let assessment = try #require(
        FitVerdict.assess(footprintMB: 8192, totalMemoryBytes: UInt64(16) << 30))
    #expect(assessment.requiredBytes == Int64(10240) << 20)
    #expect(assessment.verdict == .runsWell)
    #expect(ByteFormat.string(assessment.requiredBytes) == "10 GB")
}

@Test func fitVerdictAppliesOverheadFactorToFootprint() throws {
    let assessment = try #require(
        FitVerdict.assess(footprintMB: 400, totalMemoryBytes: machineMemory))
    #expect(assessment.requiredBytes == Int64(500) << 20)
}

@Test func missingOrEmptyFootprintYieldsNoVerdict() {
    #expect(FitVerdict.assess(footprintMB: nil, totalMemoryBytes: machineMemory) == nil)
    #expect(FitVerdict.assess(footprintMB: 0, totalMemoryBytes: machineMemory) == nil)
    #expect(FitVerdict.assess(footprintMB: -5, totalMemoryBytes: machineMemory) == nil)
    #expect(FitVerdict.assess(footprintMB: 4096, totalMemoryBytes: 0) == nil)
}

@Test func recordFitReadsFootprintAgainstMachineMemory() {
    let sized = Fixtures.flux()
    let assessment = sized.fit
    #expect(assessment != nil)
    #expect(assessment?.requiredBytes == Int64(Double(34000) * Double(1 << 20) * 1.25))

    let unsized = Fixtures.gguf()
    #expect(unsized.footprintMB == nil)
    #expect(unsized.fit == nil)
}
