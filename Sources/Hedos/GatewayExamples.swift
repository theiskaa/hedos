import Foundation
import HedosKernel

enum GatewayExamples {
    static let tokenPlaceholder = "$TOKEN"

    static func baseURL(port: Int) -> String {
        GatewayDefaults.baseURL(port: port)
    }

    static func chatCurl(port: Int, model: String, token: String) -> String {
        """
        curl -N http://127.0.0.1:\(port)/v1/chat/completions \\
          -H "Authorization: Bearer \(token)" \\
          -H "Content-Type: application/json" \\
          -d '{"model":"\(model)","messages":[{"role":"user","content":"say hello"}],"stream":true}'
        """
    }

    static func modelsCurl(port: Int, token: String) -> String {
        """
        curl http://127.0.0.1:\(port)/v1/models \\
          -H "Authorization: Bearer \(token)"
        """
    }

    static func speechCurl(port: Int, model: String, token: String) -> String {
        """
        curl http://127.0.0.1:\(port)/v1/audio/speech \\
          -H "Authorization: Bearer \(token)" \\
          -H "Content-Type: application/json" \\
          -d '{"model":"\(model)","input":"Hello from Hedos."}' \\
          -o speech.wav
        """
    }

    static func imagesCurl(port: Int, model: String, token: String) -> String {
        """
        curl http://127.0.0.1:\(port)/v1/images/generations \\
          -H "Authorization: Bearer \(token)" \\
          -H "Content-Type: application/json" \\
          -d '{"model":"\(model)","prompt":"a calm desk at dawn"}'
        """
    }

    static func pipelineRunCurl(port: Int, pipelineID: String, token: String) -> String {
        """
        curl -N http://127.0.0.1:\(port)/v1/pipelines/run \\
          -H "Authorization: Bearer \(token)" \\
          -H "Content-Type: application/json" \\
          -d '{"pipeline":"\(pipelineID)","input":{"text":"say hello"}}'
        """
    }

    static func openAISDK(port: Int, model: String, token: String) -> String {
        """
        from openai import OpenAI

        client = OpenAI(
            base_url="http://127.0.0.1:\(port)/v1",
            api_key="\(token)",
        )
        stream = client.chat.completions.create(
            model="\(model)",
            messages=[{"role": "user", "content": "say hello"}],
            stream=True,
        )
        for chunk in stream:
            print(chunk.choices[0].delta.content or "", end="", flush=True)
        """
    }

    static func ollamaClient(port: Int, model: String, token: String) -> String {
        """
        from ollama import Client

        client = Client(
            host="http://127.0.0.1:\(port)",
            headers={"Authorization": "Bearer \(token)"},
        )
        for part in client.chat(
            model="\(model)",
            messages=[{"role": "user", "content": "say hello"}],
            stream=True,
        ):
            print(part.message.content or "", end="", flush=True)
        """
    }
}
