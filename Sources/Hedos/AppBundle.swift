import Foundation
import HedosKernel

extension Bundle {
    nonisolated static let appModule: Bundle =
        ModuleBundleLocator.locate(named: "hedos_Hedos") ?? .module
}
