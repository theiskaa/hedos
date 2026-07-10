import Foundation

public struct ModelHabitat: Sendable {
    public var home: URL
    public var environment: [String: String]

    public init(
        home: URL = FileManager.default.homeDirectoryForCurrentUser,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        self.home = home
        self.environment = environment
    }

    public func roots(models: ModelsSettings) -> [(kind: SourceKind, url: URL)] {
        var roots: [(kind: SourceKind, url: URL)] = []
        roots.append((.ollama, home.appendingPathComponent(".ollama/models")))
        for url in HFCacheScanner.defaultRoots(
            environment: environment, user: models.hfCacheRoots, home: home)
        {
            roots.append((.huggingfaceCache, url))
        }
        for url in LMStudioScanner.defaultRoots(home: home) {
            roots.append((.lmStudio, url))
        }
        for url in LooseFileScanner.defaultDirectories(home: home) {
            roots.append((.file, url))
        }
        for path in models.watchedFolders {
            roots.append((.file, URL(fileURLWithPath: path, isDirectory: true)))
        }
        return roots
    }

    public func scanners(
        kinds: Set<SourceKind>?, models: ModelsSettings
    ) -> [any StoreScanner] {
        let wanted: (Set<SourceKind>) -> Bool = { scannerKinds in
            guard let kinds else { return true }
            return !scannerKinds.isDisjoint(with: kinds)
        }
        var scanners: [any StoreScanner] = []
        if wanted([.ollama]) {
            scanners.append(OllamaStoreScanner(root: home.appendingPathComponent(".ollama/models")))
        }
        if wanted([.huggingfaceCache]) {
            scanners.append(
                HFCacheScanner(
                    roots: HFCacheScanner.defaultRoots(environment: environment, home: home),
                    userRoots: HFCacheScanner.userRoots(models.hfCacheRoots)))
        }
        if wanted([.lmStudio]) {
            scanners.append(LMStudioScanner(roots: LMStudioScanner.defaultRoots(home: home)))
        }
        if wanted([.file, .folder]) {
            scanners.append(
                LooseFileScanner(
                    directories: LooseFileScanner.defaultDirectories(home: home),
                    userDirectories: models.watchedFolders.map {
                        URL(fileURLWithPath: $0, isDirectory: true)
                    }))
        }
        if wanted([.builtin]) {
            scanners.append(AppleFoundationScanner())
        }
        return scanners
    }
}
