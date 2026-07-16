public enum ByteFormat {
    public static func string(_ bytes: Int64) -> String {
        switch bytes {
        case (1 << 30)...:
            let value = Double(bytes) / Double(1 << 30)
            let formatted = String(format: "%.1f", value)
            return "\(formatted.hasSuffix(".0") ? String(formatted.dropLast(2)) : formatted) GB"
        case (1 << 20)...:
            return "\(bytes >> 20) MB"
        case 1024...:
            return "\(bytes >> 10) KB"
        default:
            return "\(bytes) B"
        }
    }
}
