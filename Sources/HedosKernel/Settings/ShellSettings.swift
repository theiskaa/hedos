import Foundation

public struct ShellSettings: SettingsDomain {
    public static let domainName = "shell"

    public var shell: ShellState

    public init() {
        shell = ShellState()
    }

    public init(shell: ShellState) {
        self.shell = shell
    }

    enum CodingKeys: String, CodingKey {
        case shell
    }

    public init(from decoder: any Decoder) throws {
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = Self()
            return
        }
        shell = container.lenient(ShellState.self, .shell, fallback: ShellState())
    }

    public static func compatibilityRead(from directory: URL) -> ShellSettings? {
        struct Legacy: Decodable {
            var shell: ShellState?
        }
        let legacy = directory.appendingPathComponent("settings.json")
        guard let data = try? Data(contentsOf: legacy),
            let value = try? JSONDecoder().decode(Legacy.self, from: data),
            let shell = value.shell
        else { return nil }
        return ShellSettings(shell: shell)
    }
}
