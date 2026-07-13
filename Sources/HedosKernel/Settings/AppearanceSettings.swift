import Foundation

public struct AppearanceSettings: SettingsDomain {
    public static let domainName = "appearance"

    public enum Theme: String, Codable, Sendable, CaseIterable {
        case system, light, dark
    }

    public enum ChatWidth: String, Codable, Sendable, CaseIterable {
        case comfortable, wide
    }

    public enum Density: String, Codable, Sendable, CaseIterable {
        case relaxed, compact
    }

    public var family: String
    public var theme: Theme
    public var chatWidth: ChatWidth
    public var density: Density
    public var uiFont: String?
    public var monoFont: String?

    public static let defaultFamily = "default"

    public init() {
        family = Self.defaultFamily
        theme = .system
        chatWidth = .comfortable
        density = .relaxed
        uiFont = nil
        monoFont = nil
    }

    public init(
        family: String = defaultFamily,
        theme: Theme = .system,
        chatWidth: ChatWidth = .comfortable,
        density: Density = .relaxed,
        uiFont: String? = nil,
        monoFont: String? = nil
    ) {
        self.family = family
        self.theme = theme
        self.chatWidth = chatWidth
        self.density = density
        self.uiFont = uiFont
        self.monoFont = monoFont
    }

    enum CodingKeys: String, CodingKey {
        case family, theme, chatWidth, density, uiFont, monoFont
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        family = container.lenient(String.self, .family, fallback: defaults.family)
        theme = container.lenient(Theme.self, .theme, fallback: defaults.theme)
        chatWidth = container.lenient(ChatWidth.self, .chatWidth, fallback: defaults.chatWidth)
        density = container.lenient(Density.self, .density, fallback: defaults.density)
        uiFont = container.lenient(String.self, .uiFont)
        monoFont = container.lenient(String.self, .monoFont)
    }
}
