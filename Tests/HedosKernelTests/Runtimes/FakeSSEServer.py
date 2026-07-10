import json
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

mode = sys.argv[1] if len(sys.argv) > 1 else "open"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass

    def authorized(self):
        return self.headers.get("Authorization", "").startswith("Bearer ")

    def do_GET(self):
        if self.path != "/v1/models":
            self.send_response(404)
            self.end_headers()
            return
        if mode == "locked" and not self.authorized():
            self.send_response(401)
            self.end_headers()
            return
        body = json.dumps(
            {
                "data": [
                    {"id": "fake-chat-1"},
                    {"id": "auth-ok" if self.authorized() else "anon"},
                ]
            }
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        if self.path != "/v1/chat/completions":
            self.send_response(404)
            self.end_headers()
            return
        if mode == "locked" and not self.authorized():
            self.send_response(401)
            self.end_headers()
            return
        if mode == "badrequest":
            self.send_response(400)
            self.end_headers()
            return
        length = int(self.headers.get("Content-Length", 0))
        request = json.loads(self.rfile.read(length) or b"{}")

        if mode == "slow":
            time.sleep(2)

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def event(payload):
            self.wfile.write(b"data: " + json.dumps(payload).encode() + b"\n\n")

        if mode == "huge":
            chunk = b"x" * (1 << 16)
            try:
                self.wfile.write(b"data: ")
                for _ in range(200):
                    self.wfile.write(chunk)
            except (BrokenPipeError, ConnectionResetError):
                pass
            return

        event({"choices": [{"delta": {"role": "assistant"}}]})
        event({"choices": [{"delta": {"content": "Hello"}}]})
        event({"choices": [{"delta": {"content": " from " + request.get("model", "?")}}]})
        event(
            {
                "choices": [{"delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 7, "completion_tokens": 4},
            }
        )
        self.wfile.write(b"data: [DONE]\n\n")


server = HTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()
