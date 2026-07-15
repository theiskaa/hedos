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

@Test func documentBudgetScalesWithTheContextWindow() {
    #expect(AttachmentIntake.documentBudget(contextLength: nil) == 32_768)
    #expect(AttachmentIntake.documentBudget(contextLength: 0) == 32_768)
    #expect(AttachmentIntake.documentBudget(contextLength: 2048) == 16_384)
    #expect(AttachmentIntake.documentBudget(contextLength: 32_768) == 65_536)
    #expect(AttachmentIntake.documentBudget(contextLength: 1_000_000) == 262_144)
}
