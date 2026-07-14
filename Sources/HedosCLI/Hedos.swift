import ArgumentParser
import Darwin
import Foundation

@main
struct Hedos: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "hedos",
        abstract: "Run and serve local models headlessly — the app's twin for scripting and gateways.",
        version: "0.1.0",
        subcommands: [
            Scan.self,
            Ls.self,
            Run.self,
            Chat.self,
            Speak.self,
            Image.self,
            Warm.self,
            Unload.self,
            Serve.self,
            Token.self,
            Pull.self,
        ])

    static func main() async {
        do {
            var command = try parseAsRoot()
            if var asyncCommand = command as? AsyncParsableCommand {
                try await asyncCommand.run()
            } else {
                try command.run()
            }
        } catch {
            let code = exitCode(for: error)
            let message = fullMessage(for: error)
            if !message.isEmpty {
                let handle = code.rawValue == 0
                    ? FileHandle.standardOutput : FileHandle.standardError
                handle.write(Data((message + "\n").utf8))
            }
            _exit(code.rawValue)
        }
        _exit(0)
    }
}
