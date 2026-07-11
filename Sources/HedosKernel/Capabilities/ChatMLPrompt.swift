enum ChatMLPrompt {
    static let noTemplateNotice = "this model has no chat template — using a generic format"

    static func render(_ messages: [ChatMessage]) -> String {
        var prompt = ""
        for message in messages {
            prompt += "<|im_start|>\(message.role.rawValue)\n\(message.content)<|im_end|>\n"
        }
        prompt += "<|im_start|>assistant\n"
        return prompt
    }
}
