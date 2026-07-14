import AppKit
import CryptoKit
import Foundation

private enum UpdateError: LocalizedError {
    case http(Int)

    var errorDescription: String? {
        switch self {
        case .http(let code): return "The server returned HTTP \(code)."
        }
    }
}

@Observable @MainActor
final class Updater {
    static let shared = Updater()

    var available: Release?

    private let repo = "theiskaa/hedos"
    private let checkInterval: TimeInterval = 60 * 60 * 24
    private var busy = false
    private var installing = false

    struct Release {
        let version: [Int]
        let tag: String
        let notes: String
        let dmg: URL
        let sha: URL?
    }

    var currentVersion: [Int] {
        Self.parse(
            Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0")
    }

    func checkOnLaunch() {
        guard !UserDefaults.standard.bool(forKey: "update.disabled") else { return }
        guard currentVersion != [0] else { return }
        let last = UserDefaults.standard.double(forKey: "update.lastCheck")
        guard Date().timeIntervalSince1970 - last > checkInterval else { return }
        Task { await check(userInitiated: false) }
    }

    func checkFromMenu() {
        Task { await check(userInitiated: true) }
    }

    private func check(userInitiated: Bool) async {
        guard !busy else { return }
        busy = true
        defer { busy = false }
        do {
            let latest = try await latestRelease()
            UserDefaults.standard.set(Date().timeIntervalSince1970, forKey: "update.lastCheck")
            guard let release = latest, isNewer(release.version, than: currentVersion) else {
                available = nil
                if userInitiated { upToDate() }
                return
            }
            if !userInitiated,
                UserDefaults.standard.string(forKey: "update.skip") == release.tag
            {
                available = nil
                return
            }
            available = release
            prompt(release, userInitiated: userInitiated)
        } catch {
            if userInitiated {
                inform("Couldn’t check for updates", error.localizedDescription)
            }
        }
    }

    func installAvailable() {
        guard let release = available else { return }
        prompt(release, userInitiated: true)
    }

    private func latestRelease() async throws -> Release? {
        var request = URLRequest(
            url: URL(string: "https://api.github.com/repos/\(repo)/releases/latest")!)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        if status == 404 { return nil }
        guard status == 200 else { throw UpdateError.http(status) }
        guard let json = try JSONSerialization.jsonObject(with: data) as? [String: Any],
            let tag = json["tag_name"] as? String,
            let assets = json["assets"] as? [[String: Any]]
        else { return nil }

        func asset(_ name: String) -> URL? {
            for entry in assets where (entry["name"] as? String) == name {
                if let link = entry["browser_download_url"] as? String { return URL(string: link) }
            }
            return nil
        }
        guard let dmg = asset("Hedos.dmg") else { return nil }
        return Release(
            version: Self.parse(tag), tag: tag, notes: (json["body"] as? String) ?? "",
            dmg: dmg, sha: asset("Hedos.dmg.sha256"))
    }

    private func prompt(_ release: Release, userInitiated: Bool) {
        let alert = NSAlert()
        alert.messageText = "Hedos \(Self.string(release.version)) is available"
        alert.informativeText =
            "You have \(Self.string(currentVersion)). Download and install it now?\n\n"
            + excerpt(release.notes)
        alert.addButton(withTitle: "Install Update")
        alert.addButton(withTitle: "Later")
        if !userInitiated { alert.addButton(withTitle: "Skip This Version") }
        switch alert.runModal() {
        case .alertFirstButtonReturn:
            Task { await downloadAndInstall(release) }
        case .alertThirdButtonReturn:
            UserDefaults.standard.set(release.tag, forKey: "update.skip")
            available = nil
        default:
            break
        }
    }

    private func downloadAndInstall(_ release: Release) async {
        guard !installing else { return }
        installing = true
        defer { installing = false }

        let panel = ProgressPanel(title: "Downloading Hedos \(Self.string(release.version))…")
        panel.show()
        do {
            let dmg = try await download(release.dmg)
            if let shaURL = release.sha, let expected = try? await expectedSHA(shaURL) {
                guard try sha256(of: dmg) == expected else {
                    try? FileManager.default.removeItem(at: dmg)
                    panel.close()
                    inform("Update failed", "The download did not pass its integrity check.")
                    return
                }
            }
            panel.close()
            try installAndRelaunch(dmg: dmg)
        } catch {
            panel.close()
            inform("Update failed", error.localizedDescription)
        }
    }

    private func download(_ url: URL) async throws -> URL {
        let (temp, response) = try await URLSession.shared.download(from: url)
        let status = (response as? HTTPURLResponse)?.statusCode ?? 0
        guard status == 200 else {
            try? FileManager.default.removeItem(at: temp)
            throw UpdateError.http(status)
        }
        let destination = FileManager.default.temporaryDirectory
            .appendingPathComponent("Hedos-update-\(UUID().uuidString).dmg")
        try? FileManager.default.removeItem(at: destination)
        try FileManager.default.moveItem(at: temp, to: destination)
        return destination
    }

    private func expectedSHA(_ url: URL) async throws -> String? {
        let (data, response) = try await URLSession.shared.data(from: url)
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
        let token = String(data: data, encoding: .utf8)?
            .split(whereSeparator: \.isWhitespace).first
            .map { $0.lowercased() }
        guard let token, token.count == 64, token.allSatisfy(\.isHexDigit) else { return nil }
        return token
    }

    private func sha256(of file: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: file)
        defer { try? handle.close() }
        var hasher = SHA256()
        while case let chunk = handle.readData(ofLength: 1 << 20), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    private func installAndRelaunch(dmg: URL) throws {
        let app = Bundle.main.bundleURL
        guard verifyAuthentic(dmg: dmg, matching: app) else {
            try? FileManager.default.removeItem(at: dmg)
            inform(
                "Update blocked",
                "The downloaded update isn’t signed by the same developer as this app, so it wasn’t installed.")
            return
        }
        guard FileManager.default.isWritableFile(atPath: app.deletingLastPathComponent().path) else {
            NSWorkspace.shared.open(dmg)
            inform("Almost there", "Drag Hedos onto Applications to finish updating.")
            return
        }

        let script = """
            #!/bin/bash
            PID="$1"; DMG="$2"; APP="$3"; MNT=""
            trap 'rm -rf "$DMG" "$0" "$MNT"' EXIT
            for _ in $(seq 1 150); do kill -0 "$PID" 2>/dev/null || break; sleep 0.2; done
            if kill -0 "$PID" 2>/dev/null; then kill "$PID" 2>/dev/null; sleep 2; kill -9 "$PID" 2>/dev/null; sleep 0.5; fi
            MNT=$(mktemp -d)
            if hdiutil attach "$DMG" -nobrowse -readonly -mountpoint "$MNT" -quiet && [ -d "$MNT/Hedos.app" ]; then
                rm -rf "$APP.old"
                if mv "$APP" "$APP.old"; then
                    if ditto "$MNT/Hedos.app" "$APP"; then rm -rf "$APP.old"; else rm -rf "$APP"; mv "$APP.old" "$APP"; fi
                fi
            fi
            hdiutil detach "$MNT" -quiet 2>/dev/null || true
            open "$APP" 2>/dev/null || open "$APP.old" 2>/dev/null || true
            """
        let scriptURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("hedos-update-\(UUID().uuidString).sh")
        try script.write(to: scriptURL, atomically: true, encoding: .utf8)

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/bash")
        process.arguments = [
            scriptURL.path, String(ProcessInfo.processInfo.processIdentifier), dmg.path, app.path,
        ]
        try process.run()
        NSApp.terminate(nil)
    }

    private func verifyAuthentic(dmg: URL, matching app: URL) -> Bool {
        let mount = FileManager.default.temporaryDirectory
            .appendingPathComponent("hedos-verify-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: mount, withIntermediateDirectories: true)
        defer {
            _ = Self.run("/usr/bin/hdiutil", ["detach", mount.path, "-force", "-quiet"])
            try? FileManager.default.removeItem(at: mount)
        }
        guard
            Self.run(
                "/usr/bin/hdiutil",
                ["attach", dmg.path, "-nobrowse", "-readonly", "-mountpoint", mount.path, "-quiet"]
            ).code == 0
        else { return false }

        let newApp = mount.appendingPathComponent("Hedos.app").path
        guard FileManager.default.fileExists(atPath: newApp),
            Self.run("/usr/bin/codesign", ["--verify", "--deep", "--strict", newApp]).code == 0
        else { return false }

        if let team = teamIdentifier(atPath: app.path) {
            return teamIdentifier(atPath: newApp) == team
        }
        return Self.run("/usr/sbin/spctl", ["--assess", "--type", "execute", newApp]).code == 0
    }

    private func teamIdentifier(atPath path: String) -> String? {
        let output = Self.run("/usr/bin/codesign", ["-dvv", path]).err
        for line in output.split(separator: "\n") where line.hasPrefix("TeamIdentifier=") {
            let value = line.dropFirst("TeamIdentifier=".count).trimmingCharacters(in: .whitespaces)
            return value == "not set" ? nil : value
        }
        return nil
    }

    private static func run(_ path: String, _ arguments: [String]) -> (code: Int32, err: String) {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = arguments
        process.standardOutput = FileHandle.nullDevice
        let errPipe = Pipe()
        process.standardError = errPipe
        guard (try? process.run()) != nil else { return (-1, "") }
        let err = errPipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return (process.terminationStatus, String(data: err, encoding: .utf8) ?? "")
    }

    private func upToDate() {
        inform("You’re up to date", "Hedos \(Self.string(currentVersion)) is the latest version.")
    }

    private func inform(_ title: String, _ text: String) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = text
        alert.runModal()
    }

    private func excerpt(_ notes: String) -> String {
        var body = notes
        if let heading = body.range(of: "\n#") {
            body = String(body[..<heading.lowerBound])
        }
        let trimmed = body.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.count > 400 ? String(trimmed.prefix(400)) + "…" : trimmed
    }

    func isNewer(_ candidate: [Int], than base: [Int]) -> Bool {
        for index in 0..<max(candidate.count, base.count) {
            let left = index < candidate.count ? candidate[index] : 0
            let right = index < base.count ? base[index] : 0
            if left != right { return left > right }
        }
        return false
    }

    static func parse(_ text: String) -> [Int] {
        let core = text.split(separator: "-").first.map(String.init) ?? text
        return core.split(separator: ".").map { Int($0.filter(\.isNumber)) ?? 0 }
    }

    static func string(_ version: [Int]) -> String {
        version.map(String.init).joined(separator: ".")
    }
}

@MainActor
private final class ProgressPanel {
    private let window: NSWindow

    init(title: String) {
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 340, height: 96),
            styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.title = "Software Update"

        let label = NSTextField(labelWithString: title)
        let bar = NSProgressIndicator()
        bar.style = .bar
        bar.isIndeterminate = true
        bar.startAnimation(nil)
        bar.widthAnchor.constraint(equalToConstant: 300).isActive = true

        let stack = NSStackView(views: [label, bar])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 20, left: 20, bottom: 20, right: 20)
        window.contentView = stack
        window.center()
    }

    func show() {
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    func close() {
        window.close()
    }
}
