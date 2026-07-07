import Foundation
import Testing

@testable import HedosKernel

@Test func promptCRUDRoundTripsAcrossStoreReload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = PromptStore(directory: dir)

    let summarize = try await store.save(
        Prompt(title: "Summarize", body: "Summarize this: {selection}", capability: .chat))
    let translate = try await store.save(
        Prompt(title: "Translate", body: "Translate {selection} to {language}"))

    #expect(await store.list().map(\.title) == ["Summarize", "Translate"])
    #expect(await store.get(id: summarize.id)?.capability == .chat)
    #expect(await store.get(id: translate.id)?.capability == nil)

    var renamed = summarize
    renamed.title = "Digest"
    renamed.body = "Digest this: {selection}"
    try await store.save(renamed)

    let reloaded = PromptStore(directory: dir)
    let listed = await reloaded.list()
    #expect(listed.map(\.title) == ["Digest", "Translate"])
    let fetched = try #require(await reloaded.get(id: summarize.id))
    #expect(fetched.body == "Digest this: {selection}")
    #expect(abs(fetched.createdAt.timeIntervalSince(summarize.createdAt)) < 1)

    await reloaded.delete(id: translate.id)
    #expect(await reloaded.list().count == 1)
    #expect(!FileManager.default.fileExists(atPath: dir.appendingPathComponent("\(translate.id).json").path))
    #expect(await PromptStore(directory: dir).list().map(\.title) == ["Digest"])
}

@Test func promptFilesArePrettyPrintedJSON() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let store = PromptStore(directory: dir)
    let prompt = try await store.save(Prompt(title: "Fix", body: "Fix grammar in {selection}"))

    let raw = try String(
        contentsOf: dir.appendingPathComponent("\(prompt.id).json"), encoding: .utf8)
    #expect(raw.contains("\n"))
    #expect(raw.contains("\"title\" : \"Fix\""))
}

@Test func placeholdersResolveAtInsertTime() {
    let prompt = Prompt(
        title: "Review",
        body: "Review {selection} as {persona}, focus on {selection}")

    #expect(prompt.placeholderNames == ["selection", "persona"])

    let resolved = prompt.resolvedBody(["selection": "the diff", "persona": "a librarian"])
    #expect(resolved == "Review the diff as a librarian, focus on the diff")

    let partial = prompt.resolvedBody(["selection": "the diff"])
    #expect(partial == "Review the diff as {persona}, focus on the diff")
}

@Test func handEditedFileWithUnknownFieldsSurvivesReload() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    let handEdited = """
        {
          "title": "Explain",
          "body": "Explain {selection} simply",
          "color": "blue",
          "tags": ["teaching", 42]
        }
        """
    try handEdited.write(
        to: dir.appendingPathComponent("explain.json"), atomically: true, encoding: .utf8)

    let store = PromptStore(directory: dir)
    let prompt = try #require(await store.get(id: "explain"))
    #expect(prompt.title == "Explain")
    #expect(prompt.body == "Explain {selection} simply")
    #expect(prompt.capability == nil)
    #expect(prompt.placeholderNames == ["selection"])

    var updated = prompt
    updated.title = "Explain simply"
    try await store.save(updated)
    let reloaded = try #require(await PromptStore(directory: dir).get(id: "explain"))
    #expect(reloaded.title == "Explain simply")
}

@Test func kernelPromptVerbsAndResolution() async throws {
    let dir = try Fixtures.tempDirectory()
    defer { try? FileManager.default.removeItem(at: dir) }
    let kernel = Kernel(directory: dir, adapters: [])

    let prompt = try await kernel.savePrompt(
        Prompt(title: "Ask", body: "Answer about {selection}", capability: .chat))
    #expect(await kernel.prompts().count == 1)
    #expect(await kernel.prompt(id: prompt.id)?.title == "Ask")

    let resolved = try await kernel.resolvePrompt(
        id: prompt.id, placeholders: ["selection": "lighthouses"])
    #expect(resolved == "Answer about lighthouses")

    await kernel.deletePrompt(id: prompt.id)
    #expect(await kernel.prompts().isEmpty)

    await #expect(throws: KernelError.self) {
        _ = try await kernel.resolvePrompt(id: prompt.id)
    }
}

@Test func composerTokenDetectionRespectsWordBoundaries() {
    #expect(PromptComposer.query(in: "/") == "")
    #expect(PromptComposer.query(in: "/sum") == "sum")
    #expect(PromptComposer.query(in: "draft text /sum") == "sum")
    #expect(PromptComposer.query(in: "line one\n/tr") == "tr")
    #expect(PromptComposer.query(in: "foo/bar") == nil)
    #expect(PromptComposer.query(in: "see https://example.com") == nil)
    #expect(PromptComposer.query(in: "/summarize this") == nil)
    #expect(PromptComposer.query(in: "no slash here") == nil)
}

@Test func composerMatchRanksPrefixOverSubstringOverSubsequence() {
    #expect(PromptComposer.matchScore("", against: "Summarize") == 3)
    #expect(PromptComposer.matchScore("sum", against: "Summarize") == 0)
    #expect(PromptComposer.matchScore("mar", against: "Summarize") == 1)
    #expect(PromptComposer.matchScore("sze", against: "Summarize") == 2)
    #expect(PromptComposer.matchScore("xyz", against: "Summarize") == nil)
}

@Test func composerInsertResolvesSelectionFromTheDraft() {
    let wrap = Prompt(title: "Summarize", body: "Summarize this:\n\n{selection}")
    #expect(
        PromptComposer.inserting(wrap, into: "hello world /sum")
            == "Summarize this:\n\nhello world")
    #expect(PromptComposer.inserting(wrap, into: "/sum") == "Summarize this:\n\n")

    let plain = Prompt(title: "Greet", body: "Say hi warmly.")
    #expect(PromptComposer.inserting(plain, into: "keep this /gr") == "keep this Say hi warmly.")

    let manual = Prompt(title: "Translate", body: "Translate {selection} to {language}")
    #expect(
        PromptComposer.inserting(manual, into: "bonjour /tr")
            == "Translate bonjour to {language}")

    #expect(PromptComposer.clearingToken(from: "keep this /model") == "keep this ")
    #expect(PromptComposer.clearingToken(from: "/model") == "")
}
