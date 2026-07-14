import ArgumentParser
import Foundation
import HedosKernel

struct Token: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "token",
        abstract: "Manage gateway client tokens.",
        subcommands: [TokenNew.self, TokenList.self, TokenRevoke.self],
        defaultSubcommand: TokenList.self)
}

struct TokenNew: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "new",
        abstract: "Mint a new client token (shown once).")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "A name for this client.")
    var name: String

    @Option(name: .long, parsing: .upToNextOption, help: "Restrict to these model ids (default: all).")
    var models: [String] = []

    @Option(name: .long, parsing: .upToNextOption, help: "Restrict to these capabilities (default: all).")
    var capabilities: [String] = []

    func run() async throws {
        let kernel = Session.kernel()
        for capability in capabilities where !Capabilities.known.contains(capability) {
            throw CLIError(
                "unknown capability \"\(capability)\" — valid: "
                + Capabilities.known.sorted().joined(separator: ", "))
        }
        let scopes = GatewayScopes(
            models: models.isEmpty ? nil : models,
            capabilities: capabilities.isEmpty ? nil : capabilities)
        let creation = try await kernel.gatewayClientStore.create(name: name, scopes: scopes)

        if global.json {
            try Out.json(TokenCreated(
                id: creation.client.id, name: creation.client.name, token: creation.token))
        } else {
            Out.line(creation.token)
            Out.err("client \"\(creation.client.name)\" (\(creation.client.id)) — store this token now; it is not shown again.")
        }
    }
}

struct TokenList: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "ls",
        abstract: "List gateway client tokens.")

    @OptionGroup var global: GlobalOptions

    func run() async throws {
        let kernel = Session.kernel()
        let clients = await kernel.gatewayClientStore.list()
        let rows = clients.map {
            TokenRow(id: $0.id, name: $0.name, createdAt: $0.createdAt, lastUsedAt: $0.lastUsedAt)
        }
        if global.json {
            try Out.json(rows)
        } else if rows.isEmpty {
            Out.line("no client tokens — mint one with `hedos token new <name>`.")
        } else {
            for row in rows { Out.line("\(row.id)  ·  \(row.name)") }
        }
    }
}

struct TokenRevoke: AsyncParsableCommand {
    static let configuration = CommandConfiguration(
        commandName: "revoke",
        abstract: "Revoke a client token by id.")

    @OptionGroup var global: GlobalOptions

    @Argument(help: "The client id to revoke.")
    var id: String

    func run() async throws {
        let kernel = Session.kernel()
        let exists = await kernel.gatewayClientStore.list().contains { $0.id == id }
        guard exists else {
            throw CLIError("no client with id \(id) — run `hedos token ls` to list them.")
        }
        try await kernel.gatewayClientStore.revoke(id: id)
        if global.json {
            try Out.json(["revoked": id])
        } else {
            Out.line("revoked \(id)")
        }
    }
}

struct TokenCreated: Encodable {
    let id: String
    let name: String
    let token: String
}

struct TokenRow: Encodable {
    let id: String
    let name: String
    let createdAt: Date
    let lastUsedAt: Date?
}
