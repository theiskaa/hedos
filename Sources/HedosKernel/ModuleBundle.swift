import Foundation

public enum ModuleBundleLocator {
    public static func locate(named name: String, roots: [URL]? = nil) -> Bundle? {
        for root in roots ?? defaultRoots() {
            if let bundle = Bundle(url: root.appendingPathComponent("\(name).bundle")) {
                return bundle
            }
        }
        return nil
    }

    static func defaultRoots(
        resourceURL: URL? = Bundle.main.resourceURL,
        bundleURL: URL = Bundle.main.bundleURL,
        executableURL: URL? = Bundle.main.executableURL
    ) -> [URL] {
        var roots: [URL] = []
        if let resourceURL { roots.append(resourceURL) }
        roots.append(bundleURL)
        roots.append(bundleURL.deletingLastPathComponent().appendingPathComponent("Resources"))
        if let executableURL {
            let directory = executableURL.resolvingSymlinksInPath().deletingLastPathComponent()
            roots.append(directory)
            roots.append(
                directory.deletingLastPathComponent().appendingPathComponent("Resources"))
        }
        return roots
    }
}

extension Bundle {
    nonisolated static let kernelModule: Bundle =
        ModuleBundleLocator.locate(named: "hedos_HedosKernel") ?? .module
}
