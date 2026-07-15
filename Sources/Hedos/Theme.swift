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
    var composer: CGFloat = 22
    var unit: CGFloat = 16
    var hairline: CGFloat = 1

    var tile: CGFloat { card }
    var surface: CGFloat { card }
}

struct ThemeFamily: Sendable, Identifiable {
    var id: String
    var name: String
    var light: ThemePalette
    var dark: ThemePalette

    static let defaultID = "default"

    static let all: [ThemeFamily] = [.standard, .gruvbox, .solarized, .catppuccin]

    static func named(_ id: String) -> ThemeFamily {
        all.first { $0.id == id } ?? .standard
    }

    static let standard = ThemeFamily(
        id: defaultID,
        name: "Default",
        light: ThemePalette(
            ground: 0xF4F4F5, panel: 0xFAFAFB, card: 0xFFFFFF, card2: 0xF7F7F8,
            line: 0xE4E4E7, lineBright: 0xD4D4D8,
            text: 0x18181B, muted: 0x6B6B70, faint: 0xA1A1A6,
            accent: 0x18181B, accentDim: 0x3F3F46, onAccent: 0xFFFFFF,
            heat: 0xB4791F, error: 0xDC2626),
        dark: ThemePalette(
            ground: 0x0A0A0B, panel: 0x121214, card: 0x171719, card2: 0x1E1E21,
            line: 0x262629, lineBright: 0x37373C,
            text: 0xFAFAFA, muted: 0xA0A0A6, faint: 0x6E6E74,
            accent: 0xFAFAFA, accentDim: 0xD0D0D6, onAccent: 0x0A0A0B,
            heat: 0xE0A64B, error: 0xE5544B))

    static let gruvbox = ThemeFamily(
        id: "gruvbox",
        name: "Gruvbox",
        light: ThemePalette(
            ground: 0xFBF1C7, panel: 0xF2E5BC, card: 0xF9F5D7, card2: 0xEBDBB2,
            line: 0xD5C4A1, lineBright: 0xBDAE93,
            text: 0x3C3836, muted: 0x665C54, faint: 0x928374,
            accent: 0x3C3836, accentDim: 0x665C54, onAccent: 0xFBF1C7,
            heat: 0xB57614, error: 0x9D0006),
        dark: ThemePalette(
            ground: 0x282828, panel: 0x1D2021, card: 0x32302F, card2: 0x3C3836,
            line: 0x504945, lineBright: 0x665C54,
            text: 0xEBDBB2, muted: 0xA89984, faint: 0x928374,
            accent: 0xEBDBB2, accentDim: 0xA89984, onAccent: 0x282828,
            heat: 0xFE8019, error: 0xFB4934))

    static let solarized = ThemeFamily(
        id: "solarized",
        name: "Solarized",
        light: ThemePalette(
            ground: 0xEEE8D5, panel: 0xF4EEDA, card: 0xFDF6E3, card2: 0xF5EFDC,
            line: 0xDDD6C1, lineBright: 0xCFC8B0,
            text: 0x586E75, muted: 0x657B83, faint: 0x93A1A1,
            accent: 0x586E75, accentDim: 0x657B83, onAccent: 0xFDF6E3,
            heat: 0xB58900, error: 0xDC322F),
        dark: ThemePalette(
            ground: 0x002B36, panel: 0x063440, card: 0x073642, card2: 0x0A4653,
            line: 0x0E4A57, lineBright: 0x17505C,
            text: 0x93A1A1, muted: 0x839496, faint: 0x586E75,
            accent: 0x93A1A1, accentDim: 0x839496, onAccent: 0x002B36,
            heat: 0xB58900, error: 0xDC322F))

    static let catppuccin = ThemeFamily(
        id: "catppuccin",
        name: "Catppuccin",
        light: ThemePalette(
            ground: 0xE6E9EF, panel: 0xEFF1F5, card: 0xF7F8FB, card2: 0xEAECF2,
            line: 0xCCD0DA, lineBright: 0xBCC0CC,
            text: 0x4C4F69, muted: 0x6C6F85, faint: 0x8C8FA1,
            accent: 0x4C4F69, accentDim: 0x5C5F77, onAccent: 0xEFF1F5,
            heat: 0xDF8E1D, error: 0xD20F39),
        dark: ThemePalette(
            ground: 0x181825, panel: 0x1E1E2E, card: 0x262637, card2: 0x313244,
            line: 0x45475A, lineBright: 0x585B70,
            text: 0xCDD6F4, muted: 0xA6ADC8, faint: 0x7F849C,
            accent: 0xCDD6F4, accentDim: 0xBAC2DE, onAccent: 0x181825,
            heat: 0xFAB387, error: 0xF38BA8))
}

enum ThemeStore {
    struct Resolved: Sendable {
        var light: ThemePalette
        var dark: ThemePalette
        var shape: ThemeShape
    }

    nonisolated(unsafe) static var light = ThemeFamily.standard.light
    nonisolated(unsafe) static var dark = ThemeFamily.standard.dark
    nonisolated(unsafe) static var shape = ThemeShape()
    nonisolated(unsafe) static private(set) var familyID = ThemeFamily.defaultID
    nonisolated(unsafe) private static var cache: [String: Resolved] = [:]

    static func select(_ id: String) {
        let family = ThemeFamily.named(id)
        familyID = family.id
        let resolved = resolve(family)
        light = resolved.light
        dark = resolved.dark
        shape = resolved.shape
    }

    private static func resolve(_ family: ThemeFamily) -> Resolved {
        if let cached = cache[family.id] { return cached }
        let resolved: Resolved
        if let source = read(name: family.id) {
            let entries = ThemeTOML.parse(source)
            resolved = Resolved(
                light: ThemeTOML.applyMode(entries, mode: "light", onto: family.light),
                dark: ThemeTOML.applyMode(entries, mode: "dark", onto: family.dark),
                shape: ThemeTOML.applyShape(entries, onto: ThemeShape()))
        } else {
            resolved = Resolved(light: family.light, dark: family.dark, shape: ThemeShape())
        }
        cache[family.id] = resolved
        return resolved
    }

    private static func read(name: String) -> String? {
        let override = ("~/.config/hedos/themes/\(name).toml" as NSString).expandingTildeInPath
        if let text = try? String(contentsOfFile: override, encoding: .utf8) { return text }
        guard
            let url = Bundle.appModule.url(
                forResource: name, withExtension: "toml", subdirectory: "Resources/Themes")
                ?? Bundle.appModule.url(
                    forResource: name, withExtension: "toml", subdirectory: "Themes")
                ?? Bundle.appModule.url(forResource: "Resources/Themes/\(name)", withExtension: "toml"),
            let text = try? String(contentsOf: url, encoding: .utf8)
        else { return nil }
        return text
    }
}

enum ThemeTOML {
    static func applyMode(
        _ entries: [String: String], mode: String, onto base: ThemePalette
    ) -> ThemePalette {
        var palette = base

        func hex(_ key: String) -> Int? {
            let raw = entries["\(mode).\(key)"] ?? entries[key]
            guard let raw else { return nil }
            let cleaned = raw.trimmingCharacters(in: CharacterSet(charactersIn: "#"))
            return Int(cleaned, radix: 16)
        }

        if let v = hex("surface.ground") { palette.ground = v }
        if let v = hex("surface.panel") { palette.panel = v }
        if let v = hex("surface.card") { palette.card = v }
        if let v = hex("surface.card2") { palette.card2 = v }
        if let v = hex("surface.line") { palette.line = v }
        if let v = hex("surface.line_bright") { palette.lineBright = v }
        if let v = hex("ink.text") { palette.text = v }
        if let v = hex("ink.muted") { palette.muted = v }
        if let v = hex("ink.faint") { palette.faint = v }
        if let v = hex("accent.value") { palette.accent = v }
        if let v = hex("accent.dim") { palette.accentDim = v }
        if let v = hex("accent.on") { palette.onAccent = v }
        if let v = hex("heat.warm") { palette.heat = v }
        if let v = hex("heat.error") { palette.error = v }

        return palette
    }

    static func applyShape(_ entries: [String: String], onto base: ThemeShape) -> ThemeShape {
        var shape = base

        func num(_ key: String) -> CGFloat? {
            guard let raw = entries[key], let value = Double(raw) else { return nil }
            return CGFloat(value)
        }

        if let v = num("shape.radius_control") { shape.control = v }
        if let v = num("shape.radius_card") { shape.card = v }
        if let v = num("shape.radius_bubble") { shape.bubble = v }
        if let v = num("shape.radius_composer") { shape.composer = v }
        if let v = num("shape.radius_artifact") { shape.artifact = v }
        if let v = num("shape.unit") { shape.unit = v }
        if let v = num("shape.hairline") { shape.hairline = v }

        return shape
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

    static func parse(_ source: String) -> [String: String] {
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
