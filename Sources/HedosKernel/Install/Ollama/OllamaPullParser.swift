import Foundation

enum OllamaPullParser {
    enum Outcome: Hashable, Sendable {
        case ignored
        case status(String)
        case progress(InstallProgress)
        case success
    }

    struct Aggregator: Sendable {
        private var totals: [String: Int64] = [:]
        private var completed: [String: Int64] = [:]
        private var lastStatus: String?

        init() {}

        mutating func fold(line: String) throws -> Outcome {
            guard let data = line.data(using: .utf8),
                let decoded = try? JSONDecoder().decode(Line.self, from: data)
            else { return .ignored }
            if let error = decoded.error, !error.isEmpty {
                throw InstallError.transferFailed("ollama: \(error)")
            }
            guard let status = decoded.status, !status.isEmpty else { return .ignored }
            if status == "success" {
                return .success
            }
            if let digest = decoded.digest, let total = decoded.total {
                totals[digest] = total
                completed[digest] = decoded.completed ?? completed[digest] ?? 0
                return .progress(aggregate())
            }
            guard status != lastStatus else { return .ignored }
            lastStatus = status
            return .status(status)
        }

        private func aggregate() -> InstallProgress {
            InstallProgress(
                bytesDownloaded: completed.values.saturatingSum(),
                totalBytes: totals.values.saturatingSum(),
                totalIsPartial: true)
        }
    }

    private struct Line: Decodable {
        let status: String?
        let digest: String?
        let total: Int64?
        let completed: Int64?
        let error: String?
    }
}
