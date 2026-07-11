import Foundation

public struct VoiceSettings: SettingsDomain {
    public static let domainName = "voice"

    public var defaultVoice: String?
    public var speed: Double
    public var autoSpeak: Bool

    public init() {
        defaultVoice = nil
        speed = 1.0
        autoSpeak = false
    }

    public init(defaultVoice: String? = nil, speed: Double = 1.0, autoSpeak: Bool = false) {
        self.defaultVoice = defaultVoice
        self.speed = speed
        self.autoSpeak = autoSpeak
    }

    enum CodingKeys: String, CodingKey {
        case defaultVoice, speed, autoSpeak
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        defaultVoice = container.lenient(String.self, .defaultVoice)
        speed = container.lenient(Double.self, .speed, fallback: defaults.speed)
        autoSpeak = container.lenient(Bool.self, .autoSpeak, fallback: defaults.autoSpeak)
    }
}
