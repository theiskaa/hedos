import Foundation
import Testing

@testable import HedosKernel

struct OllamaPullParserTests {
    @Test func transcriptFoldsIntoMonotoneAggregateProgress() throws {
        var aggregator = OllamaPullParser.Aggregator()
        let transcript = [
            #"{"status":"pulling manifest"}"#,
            #"{"status":"pulling dde5aa3fc5ff","digest":"sha256:dde5aa3fc5ff","total":1000,"completed":0}"#,
            #"{"status":"pulling dde5aa3fc5ff","digest":"sha256:dde5aa3fc5ff","total":1000,"completed":400}"#,
            #"{"status":"pulling aabbccddeeff","digest":"sha256:aabbccddeeff","total":200,"completed":200}"#,
            #"{"status":"pulling dde5aa3fc5ff","digest":"sha256:dde5aa3fc5ff","total":1000,"completed":1000}"#,
            #"{"status":"verifying sha256 digest"}"#,
            #"{"status":"writing manifest"}"#,
            #"{"status":"success"}"#,
        ]
        var progress: [InstallProgress] = []
        var statuses: [String] = []
        var success = false
        for line in transcript {
            switch try aggregator.fold(line: line) {
            case .progress(let value): progress.append(value)
            case .status(let message): statuses.append(message)
            case .success: success = true
            case .ignored: break
            }
        }
        #expect(success)
        #expect(statuses == ["pulling manifest", "verifying sha256 digest", "writing manifest"])
        #expect(
            progress.last
                == InstallProgress(
                    bytesDownloaded: 1200, totalBytes: 1200, totalIsPartial: true))
        #expect(progress.allSatisfy { $0.fraction == nil })
        let downloaded = progress.map(\.bytesDownloaded)
        #expect(downloaded == downloaded.sorted())
    }

    @Test func repeatedStatusEmitsOnce() throws {
        var aggregator = OllamaPullParser.Aggregator()
        let first = try aggregator.fold(line: #"{"status":"pulling manifest"}"#)
        let second = try aggregator.fold(line: #"{"status":"pulling manifest"}"#)
        #expect(first == .status("pulling manifest"))
        #expect(second == .ignored)
    }

    @Test func errorLineThrows() {
        var aggregator = OllamaPullParser.Aggregator()
        #expect(throws: InstallError.transferFailed("ollama: pull model manifest: file does not exist")) {
            _ = try aggregator.fold(
                line: #"{"error":"pull model manifest: file does not exist"}"#)
        }
    }

    @Test func garbageLinesAreIgnored() throws {
        var aggregator = OllamaPullParser.Aggregator()
        #expect(try aggregator.fold(line: "not json") == .ignored)
        #expect(try aggregator.fold(line: "{}") == .ignored)
        #expect(try aggregator.fold(line: #"{"status":""}"#) == .ignored)
    }

    @Test func httpErrorBodyBecomesTransferFailure() {
        let body = Data(#"{"error":"model not found"}"#.utf8)
        #expect(
            OllamaInstallProvider.pullFailure(body: body, code: 404)
                == .transferFailed("ollama: model not found"))
        #expect(
            OllamaInstallProvider.pullFailure(body: Data(), code: 500)
                == .transferFailed("ollama returned HTTP 500"))
    }

    @Test func tagShapeValidation() {
        #expect(OllamaInstallProvider.isTagShaped("gemma3:4b"))
        #expect(OllamaInstallProvider.isTagShaped("gemma3"))
        #expect(OllamaInstallProvider.isTagShaped("user/model:tag"))
        #expect(!OllamaInstallProvider.isTagShaped("org/repo"))
        #expect(!OllamaInstallProvider.isTagShaped(""))
        #expect(!OllamaInstallProvider.isTagShaped("has space"))
        #expect(!OllamaInstallProvider.isTagShaped("https://ollama.com/library/gemma3"))
        #expect(!OllamaInstallProvider.isTagShaped("name:"))
        #expect(!OllamaInstallProvider.isTagShaped(":tag"))
        #expect(!OllamaInstallProvider.isTagShaped("a/b/c:d"))
    }
}
