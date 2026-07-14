import ArgumentParser
import Foundation

struct CLIError: Error, CustomStringConvertible {
    let message: String
    init(_ message: String) { self.message = message }
    var description: String { message }
}

enum Out {
    static func line(_ text: String = "") {
        FileHandle.standardOutput.write(Data((text + "\n").utf8))
    }

    static func raw(_ text: String) {
        FileHandle.standardOutput.write(Data(text.utf8))
    }

    static func err(_ text: String) {
        FileHandle.standardError.write(Data((text + "\n").utf8))
    }

    static func json<T: Encodable>(_ value: T) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        encoder.dateEncodingStrategy = .iso8601
        let data = try encoder.encode(value)
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }
}
