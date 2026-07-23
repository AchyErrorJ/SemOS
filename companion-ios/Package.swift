// swift-tools-version:5.9
// The Swift Tools Version.

import PackageDescription

let package = Package(
    name: "SemOSCompanion",
    platforms: [
        .iOS(.v16),
        .macOS(.v13),
    ],
    products: [
        .executable(
            name: "SemOSCompanion",
            targets: ["SemOSCompanion"]
        ),
    ],
    dependencies: [],
    targets: [
        .executableTarget(
            name: "SemOSCompanion",
            path: "Sources/SemOSCompanion"
        ),
        .testTarget(
            name: "SemOSCompanionTests",
            dependencies: ["SemOSCompanion"],
            path: "Tests/SemOSCompanionTests"
        ),
    ]
)
