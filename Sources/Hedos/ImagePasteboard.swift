import AppKit
import UniformTypeIdentifiers

enum ImagePasteboard {
    struct Payload {
        let data: Data
        let type: NSPasteboard.PasteboardType
        let tiff: Data?
    }

    static func copy(fileURL: URL) async -> Bool {
        guard let payload = await Task.detached(priority: .utility, operation: {
            load(fileURL: fileURL)
        }).value else { return false }
        return await MainActor.run { write(payload) }
    }

    private nonisolated static func load(fileURL: URL) -> Payload? {
        guard let data = try? Data(contentsOf: fileURL) else { return nil }
        let declared = UTType(filenameExtension: fileURL.pathExtension)
        let type = (declared?.conforms(to: .image) ?? false) ? declared! : .png
        return Payload(
            data: data,
            type: NSPasteboard.PasteboardType(type.identifier),
            tiff: NSImage(data: data)?.tiffRepresentation)
    }

    @MainActor
    private static func write(_ payload: Payload) -> Bool {
        let item = NSPasteboardItem()
        var staged = item.setData(payload.data, forType: payload.type)
        if let tiff = payload.tiff, payload.type != .tiff {
            staged = item.setData(tiff, forType: .tiff) || staged
        }
        guard staged else { return false }
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        return pasteboard.writeObjects([item])
    }
}
