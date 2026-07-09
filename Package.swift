// swift-tools-version:6.1
import PackageDescription

let package = Package(
    name: "hedos",
    platforms: [.macOS("26.0")],
    dependencies: [
        .package(url: "https://github.com/mattt/llama.swift", .upToNextMajor(from: "2.0.0")),
        .package(url: "https://github.com/apple/swift-nio.git", from: "2.88.0"),
        .package(url: "https://github.com/apple/containerization.git", from: "0.36.0"),
        .package(url: "https://github.com/ml-explore/mlx-swift", from: "0.29.0"),
        .package(url: "https://github.com/ml-explore/mlx-swift-examples", from: "2.29.0"),
    ],
    targets: [
        .target(
            name: "HedosKernel",
            dependencies: [
                .product(name: "LlamaSwift", package: "llama.swift"),
                .product(name: "NIOCore", package: "swift-nio"),
                .product(name: "NIOPosix", package: "swift-nio"),
                .product(name: "NIOHTTP1", package: "swift-nio"),
                .product(name: "Containerization", package: "containerization"),
                .product(name: "ContainerizationOCI", package: "containerization"),
                .product(name: "MLX", package: "mlx-swift"),
                .product(name: "MLXLLM", package: "mlx-swift-examples"),
                .product(name: "MLXLMCommon", package: "mlx-swift-examples"),
            ],
            resources: [.copy("Resources")]),
        .executableTarget(
            name: "Hedos",
            dependencies: ["HedosKernel"],
            resources: [.copy("Resources")]),
        .executableTarget(
            name: "hedos-probe",
            dependencies: ["HedosKernel"]),
        .testTarget(
            name: "HedosKernelTests",
            dependencies: ["HedosKernel"],
            exclude: ["Sidecar/FakeSidecar.py", "Runtimes/FakeSSEServer.py"]),
    ]
)
