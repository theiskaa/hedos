import Foundation
import Testing

@testable import HedosKernel

@Test func intakeAcceptsTextAsACanonicalDocument() {
    let verdict = AttachmentIntake.classify(
        data: Data("hello world".utf8), filename: "My Notes.TXT", budgetBytes: 1024)
    guard case .document(let attachment) = verdict else {
        Issue.record("expected a document")
        return
    }
    #expect(attachment.kind == .document)
    #expect(attachment.name == "my-notes.txt")
    #expect(attachment.mimeType == "text/plain")
    #expect(attachment.data == Data("hello world".utf8))
}

@Test func intakeKeepsKnownTextExtensions() {
    let verdict = AttachmentIntake.classify(
        data: Data("let x = 1".utf8), filename: "Parser.swift", budgetBytes: 1024)
    guard case .document(let attachment) = verdict else {
        Issue.record("expected a document")
        return
    }
    #expect(attachment.name == "parser.swift")
}

@Test func intakeNormalizesUnknownExtensionsToTxt() {
    let verdict = AttachmentIntake.classify(
        data: Data("data".utf8), filename: "capture.weird", budgetBytes: 1024)
    guard case .document(let attachment) = verdict else {
        Issue.record("expected a document")
        return
    }
    #expect(attachment.name == "capture.txt")
    #expect(attachment.mimeType == "text/plain")
}

@Test func intakeRejectsBinaries() {
    var bytes = Data("text".utf8)
    bytes.append(0)
    bytes.append(contentsOf: [1, 2, 3])
    #expect(AttachmentIntake.classify(data: bytes, filename: "a.txt", budgetBytes: 1024) == .binary)
}

@Test func intakeRejectsLateNulBytes() {
    var bytes = Data(repeating: 0x61, count: 16_384)
    bytes.append(0)
    bytes.append(contentsOf: Data(repeating: 0x62, count: 128))
    let verdict = AttachmentIntake.classify(
        data: bytes, filename: "tail.log", budgetBytes: 262_144)
    #expect(verdict == .binary)
}

@Test func intakeRejectsOversizeDocuments() {
    let big = Data(repeating: 0x61, count: 2048)
    let verdict = AttachmentIntake.classify(data: big, filename: "big.txt", budgetBytes: 1024)
    #expect(verdict == .tooLarge(limit: 1024))
}

@Test func intakeRecodesInvalidUTF8Lossily() {
    var bytes = Data("ok ".utf8)
    bytes.append(contentsOf: [0xFF, 0xFE])
    let verdict = AttachmentIntake.classify(data: bytes, filename: "odd.txt", budgetBytes: 1024)
    guard case .document(let attachment) = verdict else {
        Issue.record("expected a document")
        return
    }
    #expect(String(decoding: attachment.data, as: UTF8.self).hasPrefix("ok "))
}

@Test func documentBudgetScalesWithTheEffectiveWindow() {
    #expect(AttachmentIntake.documentBudget(effectiveWindow: nil) == 32_768)
    #expect(AttachmentIntake.documentBudget(effectiveWindow: 0) == 32_768)
    #expect(AttachmentIntake.documentBudget(effectiveWindow: 1024) == 4096)
    #expect(AttachmentIntake.documentBudget(effectiveWindow: 4096) == 8192)
    #expect(AttachmentIntake.documentBudget(effectiveWindow: 32_768) == 65_536)
    #expect(AttachmentIntake.documentBudget(effectiveWindow: 1_000_000) == 262_144)
}

@Test func kernelDocumentBudgetFallsBackForUnknownModels() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir)
    #expect(await kernel.documentBudget(modelID: "no-such-model") == AttachmentIntake.fallbackBudget)
}

@Test func intakeTranscodesBomUTF16Text() {
    let data = "hello utf16 world".data(using: .utf16)!
    let verdict = AttachmentIntake.classify(data: data, filename: "notes.txt", budgetBytes: 1024)
    guard case .document(let attachment) = verdict else {
        Issue.record("expected a document")
        return
    }
    #expect(String(decoding: attachment.data, as: UTF8.self) == "hello utf16 world")
}

@Test func intakeTranscodesBomlessUTF16Text() {
    let data = "plain ascii text saved as utf16".data(using: .utf16LittleEndian)!
    let verdict = AttachmentIntake.classify(data: data, filename: "notes.txt", budgetBytes: 1024)
    guard case .document(let attachment) = verdict else {
        Issue.record("expected a document")
        return
    }
    #expect(
        String(decoding: attachment.data, as: UTF8.self) == "plain ascii text saved as utf16")
}

@Test func intakeBudgetsTheTranscodedSizeNotTheRawSize() {
    let text = String(repeating: "a", count: 900)
    let utf16 = text.data(using: .utf16LittleEndian)!
    #expect(utf16.count == 1800)
    let verdict = AttachmentIntake.classify(data: utf16, filename: "a.txt", budgetBytes: 1024)
    guard case .document = verdict else {
        Issue.record("expected a document — transcoded size is 900 bytes")
        return
    }
}

@Test func intakeParityHeuristicIgnoresPlainUTF8() {
    let text = "ordinary utf8 text with no nulls at all, long enough to sniff"
    let verdict = AttachmentIntake.classify(
        data: Data(text.utf8), filename: "a.txt", budgetBytes: 1024)
    guard case .document(let attachment) = verdict else {
        Issue.record("expected a document")
        return
    }
    #expect(String(decoding: attachment.data, as: UTF8.self) == text)
}

@Test func intakeTranscodesBomlessBigEndianUTF16Text() {
    let data = "big endian utf16 text here".data(using: .utf16BigEndian)!
    let verdict = AttachmentIntake.classify(data: data, filename: "be.txt", budgetBytes: 1024)
    guard case .document(let attachment) = verdict else {
        Issue.record("expected a document")
        return
    }
    #expect(
        String(decoding: attachment.data, as: UTF8.self) == "big endian utf16 text here")
}

@Test func intakeShortCircuitsAbsurdlyLargePayloads() {
    let huge = Data(repeating: 0x61, count: AttachmentIntake.maximumBudget * 4 + 1)
    let verdict = AttachmentIntake.classify(data: huge, filename: "huge.txt", budgetBytes: 512)
    #expect(verdict == .tooLarge(limit: 512))
}
