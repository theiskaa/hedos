import Foundation
import Testing

@testable import HedosKernel

@Test func auditAppendAndTailRoundTrip() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = GatewayAuditLog(directory: dir)
    for index in 0..<5 {
        await log.append(
            GatewayAuditEntry(
                client: "c\(index)", clientName: "name\(index)", method: "GET",
                route: "/v1/models", outcome: "ok", status: 200, durationMs: index))
    }
    let tail = await log.tail(limit: 3)
    #expect(tail.count == 3)
    #expect(tail.map(\.client) == ["c2", "c3", "c4"])
    #expect(tail.allSatisfy { $0.outcome == "ok" })
}

@Test func flushWritesThePendingUnauthorizedAggregate() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = GatewayAuditLog(directory: dir)
    let base = Date(timeIntervalSince1970: 1_750_000_000)
    for index in 0..<4 {
        await log.appendUnauthorized(
            GatewayAuditEntry(
                ts: base.addingTimeInterval(Double(index)), method: "GET",
                route: "/v1/models", outcome: "unauthorized", status: 401, durationMs: 0))
    }
    #expect(await log.tail(limit: 10).allSatisfy { $0.detail == nil })
    await log.flush()
    let tail = await log.tail(limit: 10)
    #expect(tail.contains { $0.detail?.contains("more unauthenticated") == true })
}

@Test func auditRotatesAtSizeThresholdKeepingGenerations() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let log = GatewayAuditLog(directory: dir, maxBytes: 512)
    for index in 0..<40 {
        await log.append(
            GatewayAuditEntry(
                client: "client-\(index)", clientName: "rotation-filler", method: "POST",
                route: "/v1/chat/completions", model: "some-model", capability: "chat",
                outcome: "ok", status: 200, durationMs: 1234))
    }
    let manager = FileManager.default
    #expect(manager.fileExists(atPath: dir.appendingPathComponent("audit.jsonl").path))
    #expect(manager.fileExists(atPath: dir.appendingPathComponent("audit.1.jsonl").path))
    let contents = try manager.contentsOfDirectory(atPath: dir.path)
    let generations = contents.filter { $0.hasPrefix("audit") }
    #expect(generations.count <= 4)

    let tail = await log.tail(limit: 5)
    #expect(!tail.isEmpty)
    #expect(tail.last?.client == "client-39")
}
