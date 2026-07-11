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

    public var theme: Theme
    public var chatWidth: ChatWidth
    public var density: Density
    public var uiFont: String?
    public var monoFont: String?

    public init() {
        theme = .system
        chatWidth = .comfortable
        density = .relaxed
        uiFont = nil
        monoFont = nil
    }

    public init(
        theme: Theme = .system,
        chatWidth: ChatWidth = .comfortable,
        density: Density = .relaxed,
        uiFont: String? = nil,
        monoFont: String? = nil
    ) {
        self.theme = theme
        self.chatWidth = chatWidth
        self.density = density
        self.uiFont = uiFont
        self.monoFont = monoFont
    }

    enum CodingKeys: String, CodingKey {
        case theme, chatWidth, density, uiFont, monoFont
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        theme = container.lenient(Theme.self, .theme, fallback: defaults.theme)
        chatWidth = container.lenient(ChatWidth.self, .chatWidth, fallback: defaults.chatWidth)
        density = container.lenient(Density.self, .density, fallback: defaults.density)
        uiFont = container.lenient(String.self, .uiFont)
        monoFont = container.lenient(String.self, .monoFont)
    }
}
