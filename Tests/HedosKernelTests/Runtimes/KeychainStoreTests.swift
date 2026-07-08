import Foundation
import Testing

@testable import HedosKernel

@Test func keychainMigratesLegacyHostPortAccountToSchemeQualifiedAccount() throws {
    let store = KeychainStore()
    let legacyAccount = "127.0.0.1:39999"
    let newAccount = "http://127.0.0.1:39999"
    try? store.delete(account: legacyAccount)
    try? store.delete(account: newAccount)
    defer {
        try? store.delete(account: legacyAccount)
        try? store.delete(account: newAccount)
    }

    try store.set("legacy-secret", account: legacyAccount)

    let migrated = try store.get(account: newAccount)
    #expect(migrated == "legacy-secret")

    #expect(try store.get(account: legacyAccount) == nil)
    #expect(try store.get(account: newAccount) == "legacy-secret")
}

@Test func keychainLegacyAccountDerivationOnlyAppliesWhenAccountHasAScheme() {
    #expect(KeychainStore.legacyAccount(for: "http://127.0.0.1:11434") == "127.0.0.1:11434")
    #expect(KeychainStore.legacyAccount(for: "https://server.local") == nil)
    #expect(KeychainStore.legacyAccount(for: "127.0.0.1:11434") == nil)
}

@Test func keychainLegacyMigrationNeverCrossesFromHttpIntoAnUnrelatedHttpsAccount() throws {
    let store = KeychainStore()
    let legacyAccount = "127.0.0.1:39998"
    let httpAccount = "http://127.0.0.1:39998"
    let httpsAccount = "https://127.0.0.1:39998"
    try? store.delete(account: legacyAccount)
    try? store.delete(account: httpAccount)
    try? store.delete(account: httpsAccount)
    defer {
        try? store.delete(account: legacyAccount)
        try? store.delete(account: httpAccount)
        try? store.delete(account: httpsAccount)
    }

    try store.set("old-http-secret", account: legacyAccount)

    #expect(try store.get(account: httpsAccount) == nil)
    #expect(try store.get(account: legacyAccount) == "old-http-secret")

    #expect(try store.get(account: httpAccount) == "old-http-secret")
    #expect(try store.get(account: legacyAccount) == nil)
    #expect(try store.get(account: httpsAccount) == nil)
}
