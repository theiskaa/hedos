import Foundation

struct OllamaModelRemover: Sendable {
    let baseURL: URL
    let transport: any InstallTransport
    let binaryPresent: @Sendable () -> Bool
    let startDaemon: @Sendable () async throws -> Void

    init(
        baseURL: URL = OllamaDefaults.baseURL,
        transport: any InstallTransport = URLSessionInstallTransport(),
        binaryPresent: @escaping @Sendable () -> Bool = {
            OllamaAdapter.daemonBinary() != nil
        },
        startDaemon: (@Sendable () async throws -> Void)? = nil
    ) {
        self.baseURL = baseURL
        self.transport = transport
        self.binaryPresent = binaryPresent
        self.startDaemon =
            startDaemon ?? { try await OllamaAdapter(baseURL: baseURL).startDaemon() }
    }

    func delete(tag: String) async throws {
        if await !reachable() {
            guard binaryPresent() else {
                throw RemovalError.daemonUnavailable(
                    hint:
                        "Ollama isn't running, and hedos can't delete its models without the daemon. Install Ollama or start it, then retry."
                )
            }
            do {
                try await startDaemon()
            } catch KernelError.runtimeUnavailable(let hint) {
                throw RemovalError.daemonUnavailable(hint: hint)
            } catch let error as RemovalError {
                throw error
            } catch {
                throw RemovalError.daemonUnavailable(hint: error.localizedDescription)
            }
        }
        var request = URLRequest(url: baseURL.appendingPathComponent("api/delete"))
        request.httpMethod = "DELETE"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONSerialization.data(withJSONObject: ["model": tag])
        let body: Data
        let response: HTTPURLResponse
        do {
            (body, response) = try await transport.fetch(request)
        } catch {
            throw RemovalError.daemonDeleteFailed(
                "ollama: \(error.localizedDescription)")
        }
        switch response.statusCode {
        case 200, 404:
            return
        default:
            throw Self.deleteFailure(body: body, code: response.statusCode)
        }
    }

    private func reachable() async -> Bool {
        var request = URLRequest(url: baseURL.appendingPathComponent("api/tags"))
        request.timeoutInterval = OllamaDefaults.probeTimeout
        guard let (_, response) = try? await transport.fetch(request) else { return false }
        return response.statusCode == 200
    }

    static func deleteFailure(body: Data, code: Int) -> RemovalError {
        .daemonDeleteFailed(OllamaDefaults.errorMessage(body: body, code: code))
    }
}
