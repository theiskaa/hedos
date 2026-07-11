import Foundation

public struct QuickAskHotkey: Codable, Sendable, Hashable {
    public var keyCode: Int
    public var modifiers: Int

    public init(keyCode: Int, modifiers: Int) {
        self.keyCode = keyCode
        self.modifiers = modifiers
    }
}

public struct GeneralSettings: SettingsDomain {
    public static let domainName = "general"

    public var restoreLastSession: Bool
    public var fixedMode: AppMode?
    public var quickAskHotkey: QuickAskHotkey?
    public var menuBarItem: Bool

    public init() {
        restoreLastSession = true
        fixedMode = nil
        quickAskHotkey = nil
        menuBarItem = false
    }

    public init(
        restoreLastSession: Bool, fixedMode: AppMode? = nil,
        quickAskHotkey: QuickAskHotkey? = nil, menuBarItem: Bool = false
    ) {
        self.restoreLastSession = restoreLastSession
        self.fixedMode = fixedMode
        self.quickAskHotkey = quickAskHotkey
        self.menuBarItem = menuBarItem
    }

    enum CodingKeys: String, CodingKey {
        case restoreLastSession, fixedMode, quickAskHotkey, menuBarItem
    }

    public init(from decoder: any Decoder) throws {
        let defaults = Self()
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = defaults
            return
        }
        restoreLastSession = container.lenient(
            Bool.self, .restoreLastSession, fallback: defaults.restoreLastSession)
        fixedMode = container.lenient(AppMode.self, .fixedMode)
        quickAskHotkey = container.lenient(QuickAskHotkey.self, .quickAskHotkey)
        menuBarItem = container.lenient(
            Bool.self, .menuBarItem, fallback: defaults.menuBarItem)
    }
}
