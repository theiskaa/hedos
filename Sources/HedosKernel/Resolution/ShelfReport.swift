import Foundation

public enum ShelfReport {
    public static func render(_ explanations: [ResolutionExplanation]) -> String {
        explanations.map(line).joined(separator: "\n")
    }

    static func line(_ explanation: ResolutionExplanation) -> String {
        let record = explanation.record
        let identified = explanation.identified
        let fields = [
            record.displayName,
            record.source.kind.rawValue,
            identified.format.rawValue,
            identified.pipelineClass ?? "-",
            identified.modality?.rawValue ?? "unidentified",
            outcome(explanation),
        ]
        return fields.joined(separator: " · ")
    }

    static func outcome(_ explanation: ResolutionExplanation) -> String {
        let record = explanation.record
        if record.state == .missing { return "missing" }
        if record.runtime.resolved == .user, let id = record.runtime.id {
            return "\(id) (\(record.runtime.tier.rawValue), user-pinned)"
        }
        guard let first = explanation.bids.first else {
            return "no bid — \(noBidReason(explanation.identified))"
        }
        var text = "\(first.adapterID) (\(first.tier.rawValue))"
        let alternatives = explanation.bids.dropFirst().map(\.adapterID)
        if !alternatives.isEmpty {
            text += " · alternatives: \(alternatives.map(\.rawValue).joined(separator: ", "))"
        }
        return text
    }

    static func noBidReason(_ identified: IdentifiedModel) -> String {
        if identified.format == .diffusers, identified.modality == nil,
            identified.pipelineClass != nil
        {
            return "unrecognized diffusers pipeline class"
        }
        guard let modality = identified.modality else {
            return "could not be identified"
        }
        if identified.capabilities.isEmpty {
            return "identified (\(modality.rawValue)) but not runnable — no capabilities"
        }
        return "no adapter serves \(modality.rawValue) \(identified.format.rawValue)"
    }
}
