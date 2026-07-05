// swift-tools-version:6.1
import PackageDescription

let package = Package(
    name: "hedos",
    platforms: [.macOS(.v15)],
    dependencies: [
        .package(url: "https://github.com/mattt/llama.swift", .upToNextMajor(from: "2.0.0"))
    ],
    targets: [
        .target(
            name: "HedosKernel",
            dependencies: [.product(name: "LlamaSwift", package: "llama.swift")]),
        .executableTarget(name: "Hedos", dependencies: ["HedosKernel"]),
        .testTarget(name: "HedosKernelTests", dependencies: ["HedosKernel"]),
    ]
)
