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
    var control: CGFloat = 8
    var card: CGFloat = 12
    var artifact: CGFloat = 14
    var bubble: CGFloat = 16
    var unit: CGFloat = 16
    var hairline: CGFloat = 1

    var tile: CGFloat { card }
    var surface: CGFloat { card }
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
            ground: 0x0F0F11, panel: 0x161618, card: 0x1C1C1F, card2: 0x232327,
            line: 0x2A2A2E, lineBright: 0x3A3A40,
            text: 0xF5F5F7, muted: 0x9A9AA0, faint: 0x6A6A70,
            accent: 0xF5F5F7, accentDim: 0xC9C9CF, onAccent: 0x0F0F11,
            heat: 0xE0A64B, error: 0xE5544B),
        shape: ThemeShape())

    static let paper = Theme(
        name: "White",
        isDark: false,
        palette: ThemePalette(
            ground: 0xF4F4F5, panel: 0xFAFAFB, card: 0xFFFFFF, card2: 0xF7F7F8,
            line: 0xE4E4E7, lineBright: 0xD4D4D8,
            text: 0x18181B, muted: 0x6B6B70, faint: 0xA1A1A6,
            accent: 0x18181B, accentDim: 0x3F3F46, onAccent: 0xFFFFFF,
            heat: 0xB4791F, error: 0xDC2626),
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
        if let v = num("shape.radius_bubble") { theme.shape.bubble = v }
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
