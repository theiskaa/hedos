import Foundation

public struct PipelineSignature: Equatable, Sendable {
    public let input: PipelinePort
    public let output: PipelinePort

    public init(input: PipelinePort, output: PipelinePort) {
        self.input = input
        self.output = output
    }
}

public enum PipelineValidationError: Error, Equatable, Sendable, CustomStringConvertible {
    case empty
    case unknownCapability(index: Int, capability: Capability)
    case modelMissing(index: Int, modelID: String)
    case modelLacksCapability(index: Int, modelID: String, capability: Capability)
    case notReady(index: Int, modelID: String)
    case incompatibleEdge(from: Int, to: Int, produced: PipelinePort, expected: PipelinePort)
    case deadEndOutput(index: Int, capability: Capability)

    public var description: String {
        switch self {
        case .empty:
            return "a pipeline needs at least one stage"
        case .unknownCapability(let index, let capability):
            return "stage \(index + 1) uses \(capability.rawValue), which can't be chained"
        case .modelMissing(let index, let modelID):
            return "stage \(index + 1) points at a model that is no longer on the shelf (\(modelID))"
        case .modelLacksCapability(let index, let modelID, let capability):
            return "stage \(index + 1)'s model does not do \(capability.rawValue) (\(modelID))"
        case .notReady(let index, let modelID):
            return "stage \(index + 1)'s model isn't ready to run (\(modelID))"
        case .incompatibleEdge(let from, let to, let produced, let expected):
            return
                "stage \(from + 1) produces \(produced.rawValue) but stage \(to + 1) expects \(expected.rawValue)"
        case .deadEndOutput(let index, _):
            return "stage \(index + 1) ends the pipeline with vectors, which nothing can consume yet"
        }
    }
}

public enum PipelineValidator {
    @discardableResult
    public static func validate(
        _ stages: [PipelineStage], shelf: [ModelRecord]
    ) throws -> PipelineSignature {
        guard !stages.isEmpty else { throw PipelineValidationError.empty }

        var signatures: [CapabilitySignature] = []
        for (index, stage) in stages.enumerated() {
            guard let signature = CapabilitySignatures.signature(stage.capability) else {
                throw PipelineValidationError.unknownCapability(
                    index: index, capability: stage.capability)
            }
            guard let record = shelf.first(where: { $0.id == stage.modelID }) else {
                throw PipelineValidationError.modelMissing(index: index, modelID: stage.modelID)
            }
            guard record.capabilities.contains(stage.capability) else {
                throw PipelineValidationError.modelLacksCapability(
                    index: index, modelID: stage.modelID, capability: stage.capability)
            }
            guard record.state == .ready else {
                throw PipelineValidationError.notReady(index: index, modelID: stage.modelID)
            }
            signatures.append(signature)
        }

        for index in 0..<(signatures.count - 1) {
            let produced = signatures[index].output
            let expected = signatures[index + 1].input
            if produced != expected {
                throw PipelineValidationError.incompatibleEdge(
                    from: index, to: index + 1, produced: produced, expected: expected)
            }
        }

        if signatures.last!.output == .vector {
            throw PipelineValidationError.deadEndOutput(
                index: signatures.count - 1, capability: stages.last!.capability)
        }

        return PipelineSignature(
            input: signatures.first!.input, output: signatures.last!.output)
    }

    public static func nextCapabilities(after stages: [PipelineStage]) -> [Capability] {
        let consumable = CapabilitySignatures.composable.filter {
            CapabilitySignatures.signature($0)?.output != .vector
        }
        guard let last = stages.last,
            let signature = CapabilitySignatures.signature(last.capability)
        else {
            return consumable
        }
        return consumable.filter {
            CapabilitySignatures.signature($0)?.input == signature.output
        }
    }
}
