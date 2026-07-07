// swift-tools-version:6.1
import PackageDescription

let package = Package(
    name: "hedos",
    platforms: [.macOS("26.0")],
    dependencies: [
        .package(url: "https://github.com/mattt/llama.swift", .upToNextMajor(from: "2.0.0"))
    ],
    targets: [
        .target(
            name: "HedosKernel",
            dependencies: [.product(name: "LlamaSwift", package: "llama.swift")],
            resources: [.copy("Resources")]),
        .executableTarget(
            name: "Hedos",
            dependencies: ["HedosKernel"],
            resources: [.copy("Resources")]),
        .testTarget(
            name: "HedosKernelTests",
            dependencies: ["HedosKernel"],
            exclude: ["Sidecar/FakeSidecar.py"]),
    ]
)
