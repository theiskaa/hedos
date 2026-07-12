import Foundation
import Testing

@testable import HedosKernel

private func dataURI() -> String {
    "data:image/png;base64," + Data([0x89, 0x50, 0x4E, 0x47]).base64EncodedString()
}

@Test func openAIAcceptsADataURIImagePart() throws {
    let body: [String: Any] = [
        "model": "llava",
        "messages": [
            [
                "role": "user",
                "content": [
                    ["type": "text", "text": "what is this?"],
                    ["type": "image_url", "image_url": ["url": dataURI()]],
                ],
            ]
        ],
    ]
    let request = try OpenAIWire.decodeChatRequest(body)
    let attachments = request.messages.first?.attachments ?? []
    #expect(attachments.count == 1)
    #expect(attachments.first?.mimeType == "image/png")
    #expect(request.messages.first?.content == "what is this?")
}

@Test func openAIRejectsARemoteImageURL() {
    let body: [String: Any] = [
        "model": "llava",
        "messages": [
            [
                "role": "user",
                "content": [
                    ["type": "image_url", "image_url": ["url": "https://example.com/cat.png"]]
                ],
            ]
        ],
    ]
    #expect(throws: GatewayError.self) {
        _ = try OpenAIWire.decodeChatRequest(body)
    }
}

@Test func openAIRejectsWrongTypedSamplingScalars() {
    #expect(throws: GatewayError.self) {
        _ = try OpenAIWire.decodeSampling(["temperature": "hot"])
    }
    #expect(throws: GatewayError.self) {
        _ = try OpenAIWire.decodeSampling(["max_tokens": "lots"])
    }
    let ok = try? OpenAIWire.decodeSampling(["temperature": 0.7, "max_tokens": 8])
    #expect(ok?["temperature"] == .double(0.7))
    #expect(ok?["max_tokens"] == .int(8))
}

@Test func openAIRejectsANonStringOrObjectToolChoice() {
    let body: [String: Any] = [
        "model": "m",
        "messages": [["role": "user", "content": "hi"]],
        "tool_choice": 5,
    ]
    #expect(throws: GatewayError.self) {
        _ = try OpenAIWire.decodeChatRequest(body)
    }
    let ok: [String: Any] = [
        "model": "m", "messages": [["role": "user", "content": "hi"]], "tool_choice": "auto",
    ]
    #expect((try? OpenAIWire.decodeChatRequest(ok)) != nil)
}

@Test func ollamaReadsTheImagesArray() throws {
    let encoded = Data([0x89, 0x50, 0x4E, 0x47]).base64EncodedString()
    let body: [String: Any] = [
        "model": "llava",
        "messages": [
            ["role": "user", "content": "what is this?", "images": [encoded]]
        ],
    ]
    let request = try OllamaWire.decodeChatRequest(body)
    #expect(request.messages.first?.attachments.count == 1)
    #expect(request.messages.first?.attachments.first?.mimeType == "image/png")
}

@Test func ollamaRejectsANonBase64Image() {
    let body: [String: Any] = [
        "model": "llava",
        "messages": [
            ["role": "user", "content": "hi", "images": [123]]
        ],
    ]
    #expect(throws: GatewayError.self) {
        _ = try OllamaWire.decodeChatRequest(body)
    }
}
