import Foundation

struct ThemePalette: Sendable {
    var ground: Int
    var panel: Int
    var card: Int
    var card2: Int
    var line: Int
    var lineBright: Int
    var text: Int
    var muted: Int
    var faint: Int
    var accent: Int
    var accentDim: Int
    var onAccent: Int
    var heat: Int
    var error: Int
}

struct ThemeShape: Sendable {
    var control: CGFloat = 2
    var card: CGFloat = 3
    var artifact: CGFloat = 10
    var unit: CGFloat = 16
    var hairline: CGFloat = 1

    var tile: CGFloat { card }
    var surface: CGFloat { card }
    var bubble: CGFloat { artifact }
}

struct Theme: Sendable {
    var name: String
    var isDark: Bool
    var palette: ThemePalette
    var shape: ThemeShape

    static let graphite = Theme(
        name: "Graphite",
        isDark: true,
        palette: ThemePalette(
            ground: 0x141210, panel: 0x1A1815, card: 0x211E1E, card2: 0x26221F,
            line: 0x322E2A, lineBright: 0x4A443E,
            text: 0xF1ECEC, muted: 0xA39C94, faint: 0x6E655D,
            accent: 0xEDE8E3, accentDim: 0xCFCECD, onAccent: 0x17140F,
            heat: 0xC99A4E, error: 0xC06A4E),
        shape: ThemeShape())

    static let paper = Theme(
        name: "Paper",
        isDark: false,
        palette: ThemePalette(
            ground: 0xE9E5DE, panel: 0xF1EDE7, card: 0xFBF9F5, card2: 0xF5F1EB,
            line: 0xDBD5CB, lineBright: 0xC4BDB1,
            text: 0x211E1E, muted: 0x6E655D, faint: 0x9C948A,
            accent: 0x211E1E, accentDim: 0x4B4646, onAccent: 0xF5F1EB,
            heat: 0x9A6E22, error: 0xA24A30),
        shape: ThemeShape())
}

final class ThemeStore: Sendable {
    static let shared = ThemeStore()

    let dark: ThemePalette
    let light: ThemePalette
    let shape: ThemeShape

    private init() {
        let graphite = ThemeStore.resolve(name: "graphite", base: .graphite)
        let paper = ThemeStore.resolve(name: "paper", base: .paper)
        dark = graphite.palette
        light = paper.palette
        shape = graphite.shape
    }

    private static func resolve(name: String, base: Theme) -> Theme {
        guard let source = read(name: name) else { return base }
        return ThemeTOML.apply(source, onto: base)
    }

    private static func read(name: String) -> String? {
        let override = ("~/.config/hedos/themes/\(name).toml" as NSString).expandingTildeInPath
        if let text = try? String(contentsOfFile: override, encoding: .utf8) { return text }
        guard
            let url = Bundle.module.url(
                forResource: name, withExtension: "toml", subdirectory: "Resources/Themes")
                ?? Bundle.module.url(
                    forResource: name, withExtension: "toml", subdirectory: "Themes")
                ?? Bundle.module.url(forResource: "Resources/Themes/\(name)", withExtension: "toml"),
            let text = try? String(contentsOf: url, encoding: .utf8)
        else { return nil }
        return text
    }
}

enum ThemeTOML {
    static func apply(_ source: String, onto base: Theme) -> Theme {
        let entries = parse(source)
        var theme = base

        func hex(_ key: String) -> Int? {
            guard let raw = entries[key] else { return nil }
            let cleaned = raw.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
            return Int(cleaned, radix: 16)
        }
        func num(_ key: String) -> CGFloat? {
            guard let raw = entries[key], let value = Double(raw) else { return nil }
            return CGFloat(value)
        }

        if let v = entries["meta.name"] { theme.name = v }
        if let v = entries["meta.appearance"] { theme.isDark = v == "dark" }

        if let v = hex("surface.ground") { theme.palette.ground = v }
        if let v = hex("surface.panel") { theme.palette.panel = v }
        if let v = hex("surface.card") { theme.palette.card = v }
        if let v = hex("surface.card2") { theme.palette.card2 = v }
        if let v = hex("surface.line") { theme.palette.line = v }
        if let v = hex("surface.line_bright") { theme.palette.lineBright = v }
        if let v = hex("ink.text") { theme.palette.text = v }
        if let v = hex("ink.muted") { theme.palette.muted = v }
        if let v = hex("ink.faint") { theme.palette.faint = v }
        if let v = hex("accent.value") { theme.palette.accent = v }
        if let v = hex("accent.dim") { theme.palette.accentDim = v }
        if let v = hex("accent.on") { theme.palette.onAccent = v }
        if let v = hex("heat.warm") { theme.palette.heat = v }
        if let v = hex("heat.error") { theme.palette.error = v }

        if let v = num("shape.radius_control") { theme.shape.control = v }
        if let v = num("shape.radius_card") { theme.shape.card = v }
        if let v = num("shape.radius_artifact") { theme.shape.artifact = v }
        if let v = num("shape.unit") { theme.shape.unit = v }
        if let v = num("shape.hairline") { theme.shape.hairline = v }

        return theme
    }

    private static func stripComment(_ line: String) -> String {
        var quote: Character?
        for index in line.indices {
            let character = line[index]
            if let open = quote {
                if character == open { quote = nil }
            } else if character == "\"" || character == "'" {
                quote = character
            } else if character == "#" {
                return String(line[..<index])
            }
        }
        return line
    }

    private static func parse(_ source: String) -> [String: String] {
        var result: [String: String] = [:]
        var section = ""
        for rawLine in source.split(whereSeparator: \.isNewline) {
            var line = stripComment(String(rawLine))
            line = line.trimmingCharacters(in: .whitespaces)
            if line.isEmpty { continue }
            if line.hasPrefix("[") && line.hasSuffix("]") {
                section = String(line.dropFirst().dropLast()).trimmingCharacters(in: .whitespaces)
                continue
            }
            guard let equals = line.firstIndex(of: "=") else { continue }
            let key = line[..<equals].trimmingCharacters(in: .whitespaces)
            var value = line[line.index(after: equals)...].trimmingCharacters(in: .whitespaces)
            value = value.trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            let full = section.isEmpty ? key : "\(section).\(key)"
            result[full] = value
        }
        return result
    }
}
