import Foundation

public enum InstallError: Error, Sendable, Hashable, LocalizedError {
    case providerUnknown(InstallProviderID)
    case providerUnavailable(hint: String)
    case referenceInvalid(String)
    case referenceNotFound(String)
    case authRequired(String)
    case insufficientDisk(requiredBytes: Int64, availableBytes: Int64)
    case checksumMismatch(file: String)
    case transferFailed(String)

    public var errorDescription: String? {
        switch self {
        case .providerUnknown(let id):
            "No install provider is registered as \(id.rawValue)."
        case .providerUnavailable(let hint):
            hint
        case .referenceInvalid(let reference):
            "\(reference) is not a reference this provider understands."
        case .referenceNotFound(let reference):
            "\(reference) was not found on the platform."
        case .authRequired(let reference):
            "\(reference) is gated. Sign in with `huggingface-cli login` or set HF_TOKEN, then try again."
        case .insufficientDisk(let requiredBytes, let availableBytes):
            "Not enough free disk space: this model needs \(ByteCountFormatter.string(fromByteCount: requiredBytes, countStyle: .file)) and \(ByteCountFormatter.string(fromByteCount: availableBytes, countStyle: .file)) is available."
        case .checksumMismatch(let file):
            "\(file) failed checksum verification after download."
        case .transferFailed(let message):
            message
        }
    }
}
