import Foundation
import SQLite3

enum SQLiteValue: Equatable {
    case null
    case integer(Int64)
    case real(Double)
    case text(String)
}

struct SQLiteRow {
    let columns: [SQLiteValue]

    func text(_ index: Int) -> String {
        if case .text(let value) = columns[index] { return value }
        return ""
    }

    func optionalText(_ index: Int) -> String? {
        if case .text(let value) = columns[index] { return value }
        return nil
    }

    func integer(_ index: Int) -> Int64 {
        if case .integer(let value) = columns[index] { return value }
        return 0
    }

    func real(_ index: Int) -> Double {
        switch columns[index] {
        case .real(let value): return value
        case .integer(let value): return Double(value)
        default: return 0
        }
    }

    func optionalReal(_ index: Int) -> Double? {
        switch columns[index] {
        case .real(let value): return value
        case .integer(let value): return Double(value)
        default: return nil
        }
    }
}

final class ChatDatabase {
    private let handle: OpaquePointer
    private let writes: WriteCounter

    final class WriteCounter {
        var rowsByTable: [String: Int] = [:]
    }

    init(url: URL) throws {
        do {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        } catch {
            throw ChatStoreError.databaseUnavailable(String(describing: error))
        }
        var opened: OpaquePointer?
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX
        guard sqlite3_open_v2(url.path, &opened, flags, nil) == SQLITE_OK, let handle = opened
        else {
            let message =
                opened.map { String(cString: sqlite3_errmsg($0)) } ?? "cannot open \(url.path)"
            sqlite3_close_v2(opened)
            throw ChatStoreError.databaseUnavailable(message)
        }
        self.handle = handle
        sqlite3_busy_timeout(handle, 5000)
        let writes = WriteCounter()
        self.writes = writes
        sqlite3_update_hook(
            handle,
            { context, _, _, table, _ in
                guard let context, let table else { return }
                let counter = Unmanaged<WriteCounter>.fromOpaque(context).takeUnretainedValue()
                counter.rowsByTable[String(cString: table), default: 0] += 1
            },
            Unmanaged.passUnretained(writes).toOpaque())
    }

    deinit {
        sqlite3_close_v2(handle)
    }

    func execute(_ sql: String) throws {
        var message: UnsafeMutablePointer<CChar>?
        guard sqlite3_exec(handle, sql, nil, nil, &message) == SQLITE_OK else {
            let description = message.map { String(cString: $0) } ?? sql
            sqlite3_free(message)
            throw ChatStoreError.statementFailed(description)
        }
    }

    func transaction<Result>(_ body: () throws -> Result) throws -> Result {
        try enclosing("BEGIN IMMEDIATE", body)
    }

    func readTransaction<Result>(_ body: () throws -> Result) throws -> Result {
        try enclosing("BEGIN DEFERRED", body)
    }

    private func enclosing<Result>(_ begin: String, _ body: () throws -> Result) throws -> Result {
        try execute(begin)
        do {
            let result = try body()
            try execute("COMMIT")
            return result
        } catch {
            try? execute("ROLLBACK")
            throw error
        }
    }

    @discardableResult
    func run(_ sql: String, _ bindings: [SQLiteValue] = []) throws -> Int {
        let statement = try prepare(sql, bindings)
        defer { sqlite3_finalize(statement) }
        guard sqlite3_step(statement) == SQLITE_DONE else {
            throw ChatStoreError.statementFailed(String(cString: sqlite3_errmsg(handle)))
        }
        return Int(sqlite3_changes(handle))
    }

    func rows(_ sql: String, _ bindings: [SQLiteValue] = []) throws -> [SQLiteRow] {
        let statement = try prepare(sql, bindings)
        defer { sqlite3_finalize(statement) }
        var result: [SQLiteRow] = []
        while true {
            switch sqlite3_step(statement) {
            case SQLITE_ROW:
                result.append(readRow(statement))
            case SQLITE_DONE:
                return result
            default:
                throw ChatStoreError.statementFailed(String(cString: sqlite3_errmsg(handle)))
            }
        }
    }

    func userVersion() throws -> Int {
        Int(try rows("PRAGMA user_version").first?.integer(0) ?? 0)
    }

    func setUserVersion(_ version: Int) throws {
        try execute("PRAGMA user_version = \(version)")
    }

    func rowsWritten(to tables: Set<String>) -> Int {
        tables.reduce(0) { $0 + (writes.rowsByTable[$1] ?? 0) }
    }

    func resetWriteCounter() {
        writes.rowsByTable = [:]
    }

    private func prepare(_ sql: String, _ bindings: [SQLiteValue]) throws -> OpaquePointer {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(handle, sql, -1, &statement, nil) == SQLITE_OK, let statement
        else {
            throw ChatStoreError.statementFailed(String(cString: sqlite3_errmsg(handle)))
        }
        for (index, value) in bindings.enumerated() {
            let position = Int32(index + 1)
            let status: Int32 =
                switch value {
                case .null:
                    sqlite3_bind_null(statement, position)
                case .integer(let value):
                    sqlite3_bind_int64(statement, position, value)
                case .real(let value):
                    sqlite3_bind_double(statement, position, value)
                case .text(let value):
                    sqlite3_bind_text(statement, position, value, -1, Self.transient)
                }
            guard status == SQLITE_OK else {
                sqlite3_finalize(statement)
                throw ChatStoreError.statementFailed(String(cString: sqlite3_errmsg(handle)))
            }
        }
        return statement
    }

    private func readRow(_ statement: OpaquePointer) -> SQLiteRow {
        var columns: [SQLiteValue] = []
        for index in 0..<sqlite3_column_count(statement) {
            switch sqlite3_column_type(statement, index) {
            case SQLITE_INTEGER:
                columns.append(.integer(sqlite3_column_int64(statement, index)))
            case SQLITE_FLOAT:
                columns.append(.real(sqlite3_column_double(statement, index)))
            case SQLITE_TEXT:
                columns.append(.text(String(cString: sqlite3_column_text(statement, index))))
            default:
                columns.append(.null)
            }
        }
        return SQLiteRow(columns: columns)
    }

    private static let transient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)
}
