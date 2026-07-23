// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "HedosMlxShim",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "HedosMlxShim", type: .dynamic, targets: ["HedosMlxShim"])
    ],
    dependencies: [
        .package(url: "https://github.com/ml-explore/mlx-swift", from: "0.29.0"),
        .package(url: "https://github.com/ml-explore/mlx-swift-examples", from: "2.29.0"),
    ],
    targets: [
        .target(
            name: "HedosMlxShim",
            dependencies: [
                .product(name: "MLX", package: "mlx-swift"),
                .product(name: "MLXLLM", package: "mlx-swift-examples"),
                .product(name: "MLXLMCommon", package: "mlx-swift-examples"),
            ]
        )
    ]
)
