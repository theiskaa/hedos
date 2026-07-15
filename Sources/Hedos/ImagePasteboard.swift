import AppKit
import UniformTypeIdentifiers

enum ImagePasteboard {
    static func prefersText(_ pasteboard: NSPasteboard) -> Bool {
        guard
            let text = pasteboard.string(forType: .string)?
                .trimmingCharacters(in: .whitespacesAndNewlines),
            !text.isEmpty
        else { return false }
        let isBareWebLink =
            text.split(whereSeparator: \.isWhitespace).count == 1
            && URL(string: text).map { $0.scheme == "http" || $0.scheme == "https" } == true
        return !isBareWebLink
    }

    static func carriesAttachment(_ pasteboard: NSPasteboard) -> Bool {
        if pasteboard.canReadObject(
            forClasses: [NSURL.self], options: [.urlReadingFileURLsOnly: true])
        {
            return true
        }
        guard pasteboard.canReadObject(forClasses: [NSImage.self]) else { return false }
        return !prefersText(pasteboard)
    }

    static func pngData(_ image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation,
            let rep = NSBitmapImageRep(data: tiff)
        else { return nil }
        return rep.representation(using: .png, properties: [:])
    }

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

    @MainActor
    static func copy(image: NSImage) -> Bool {
        guard let data = pngData(image) else { return false }
        return write(
            Payload(
                data: data,
                type: NSPasteboard.PasteboardType(UTType.png.identifier),
                tiff: image.tiffRepresentation))
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
