// Swift warmup runner.
//
// This program is compiled and executed at image build time to:
//   1. Force swiftc to populate the module cache for Foundation, Dispatch,
//      and the Swift stdlib (the dominant cold-start cost on subsequent
//      compilations).
//   2. Exercise hot paths in Foundation (JSON, Data, String) so any
//      late-bound stubs are resolved before a job lands on this template.
//
// The compiled binary itself is also kept around so deployments that wire
// in a swiftc wrapper can invoke `/opt/warmup/SwiftWarmup` as a smoke
// check during pool acquisition.

import Foundation

struct Item: Codable {
    let id: Int
    let name: String
    let tags: [String]
    let payload: [String: Int]
}

struct SwiftWarmup {
    static func main() {
        let iterations: Int
        if CommandLine.arguments.count > 1, let n = Int(CommandLine.arguments[1]) {
            iterations = n
        } else {
            iterations = 100
        }

        let start = Date()
        var sink = 0
        for _ in 0..<iterations {
            sink &+= roundTrip()
            sink &+= computations()
            sink &+= collections()
        }
        let elapsedMs = Int(Date().timeIntervalSince(start) * 1000)
        print("warmup done iterations=\(iterations) elapsed_ms=\(elapsedMs) sink=\(sink)")
    }

    /// Codable round-trip exercises Foundation's JSON path.
    static func roundTrip() -> Int {
        let items = (0..<50).map { i in
            Item(
                id: i,
                name: "item-\(i)",
                tags: ["t\(i % 7)", "t\(i % 11)"],
                payload: ["a": i, "b": i * 2, "c": i * 3]
            )
        }
        let encoded = (try? JSONEncoder().encode(items)) ?? Data()
        let decoded = (try? JSONDecoder().decode([Item].self, from: encoded)) ?? []
        return encoded.count + decoded.count
    }

    /// FP-heavy loop to exercise the optimizer's vector path.
    static func computations() -> Int {
        var acc = 0.0
        for i in 0..<500 {
            acc += (Double(i)).squareRoot() * Double.random(in: 0..<1)
            acc = sin(acc) + cos(acc)
        }
        return acc > 1e10 ? 1 : 0
    }

    /// Collection ops representative of build-tool workloads.
    static func collections() -> Int {
        var dict: [String: Int] = [:]
        for i in 0..<200 { dict["k\(i)"] = i }
        let total = dict.values.reduce(0, +)
        let filtered = (0..<200)
            .map { "v\($0)" }
            .filter { $0.contains("5") }
            .count
        return total + filtered
    }
}

SwiftWarmup.main()
