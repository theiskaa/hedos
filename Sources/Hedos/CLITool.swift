import AppKit
import Foundation

enum CLITool {
    static let installPath = "/usr/local/bin/hedos"

    static var bundledBinary: URL? {
        let url = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/hedos")
        return FileManager.default.isExecutableFile(atPath: url.path) ? url : nil
    }

    static var isInstalled: Bool {
        guard let bundled = bundledBinary else { return false }
        let link = try? FileManager.default.destinationOfSymbolicLink(atPath: installPath)
        return link == bundled.path
    }

    @MainActor
    static func offerOnFirstRun() {
        let key = "cli.install.offered"
        let defaults = UserDefaults.standard
        guard !defaults.bool(forKey: key) else { return }
        defaults.set(true, forKey: key)
        guard bundledBinary != nil, !isInstalled else { return }

        let alert = NSAlert()
        alert.messageText = "Install the hedos command-line tool?"
        alert.informativeText =
            "Adds a “hedos” command to your terminal so you can scan, run, and serve models "
            + "from the shell. You can install it any time from the Hedos menu."
        alert.addButton(withTitle: "Install")
        alert.addButton(withTitle: "Not Now")
        if alert.runModal() == .alertFirstButtonReturn {
            report(install())
        }
    }

    @MainActor
    static func installFromMenu() {
        report(install())
    }

    @MainActor
    private static func install() -> Bool {
        guard let bundled = bundledBinary else { return false }
        let target = shellQuoted(bundled.path)
        return runPrivileged("mkdir -p /usr/local/bin && ln -sf \(target) \(installPath)")
    }

    @MainActor
    private static func report(_ succeeded: Bool) {
        let alert = NSAlert()
        if succeeded {
            alert.messageText = "Command-line tool installed"
            alert.informativeText = "Open a new terminal window and run “hedos --help”."
        } else {
            alert.messageText = "Couldn’t install the command-line tool"
            alert.informativeText =
                "Install it with Homebrew instead: brew install --cask hedos."
        }
        alert.runModal()
    }

    @MainActor
    private static func runPrivileged(_ command: String) -> Bool {
        let escaped = command
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
        var error: NSDictionary?
        NSAppleScript(source: "do shell script \"\(escaped)\" with administrator privileges")?
            .executeAndReturnError(&error)
        return error == nil
    }

    private static func shellQuoted(_ path: String) -> String {
        "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'"
    }
}
