// swift-tools-version:6.1
import PackageDescription

let package = Package(
    name: "hedos",
    platforms: [.macOS(.v15)],
    targets: [
        .target(name: "HedosKernel"),
        .executableTarget(name: "Hedos", dependencies: ["HedosKernel"]),
        .testTarget(name: "HedosKernelTests", dependencies: ["HedosKernel"]),
    ]
)
