import http.server
import json
import socketserver
import time

PORT = 8090

NON_STREAM = {
    "openai": {
        "id": "chatcmpl-1",
        "model": "mock",
        "choices": [
            {"index": 0, "message": {"role": "assistant", "content": "hi"}, "finish_reason": "stop"}
        ],
        "usage": {"prompt_tokens": 10, "completion_tokens": 8, "total_tokens": 18},
    },
    "anthropic": {
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "content": [{"type": "text", "text": "hi"}],
        "model": "mock",
        # No total_tokens: Anthropic's Messages API has no native total field.
        "usage": {"input_tokens": 10, "output_tokens": 8},
    },
    "gemini": {
        "candidates": [{"content": {"parts": [{"text": "hi"}]}, "finishReason": "STOP"}],
        "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 8, "totalTokenCount": 18},
    },
    # Synthetic edge cases, not a real provider shape:
    "ambiguous": {
        # First candidate present but non-numeric; exercises the `number` type hint.
        "usage": {"total_tokens": "not-a-number"},
        "usageMetadata": {"totalTokenCount": 33},
    },
    "unknown": {
        # None of the configured candidates are present; exercises the
        # no-candidate-resolves / fail-open / kuadrant.body_extraction_misses path.
        "model": "mystery-provider",
        "result": "ok",
    },
}

# Each real provider's SSE framing quirks are reproduced on purpose (see RFC 0024
# background on why a single "look at the last event" heuristic doesn't generalize):
#  - openai: bare `data:` chunks, usage on its own final chunk, `[DONE]` sentinel.
#  - anthropic: named events, usage split across message_start (input) and
#    message_delta (output), no terminal sentinel.
#  - gemini: bare `data:` chunks, usageMetadata withheld until the true last
#    chunk, no terminal sentinel.
STREAM_EVENTS = {
    "openai": [
        'data: {"id":"chatcmpl-1","choices":[{"delta":{"role":"assistant"}}]}\n\n',
        'data: {"id":"chatcmpl-1","choices":[{"delta":{"content":"hi"}}]}\n\n',
        'data: {"id":"chatcmpl-1","choices":[],'
        '"usage":{"prompt_tokens":10,"completion_tokens":8,"total_tokens":18}}\n\n',
        "data: [DONE]\n\n",
    ],
    "anthropic": [
        'event: message_start\ndata: {"type":"message_start","message":'
        '{"id":"msg_1","usage":{"input_tokens":10,"output_tokens":0}}}\n\n',
        'event: content_block_delta\ndata: {"type":"content_block_delta","delta":{"text":"hi"}}\n\n',
        'event: message_delta\ndata: {"type":"message_delta","delta":{"stop_reason":"end_turn"},'
        '"usage":{"output_tokens":8}}\n\n',
        'event: message_stop\ndata: {"type":"message_stop"}\n\n',
    ],
    "gemini": [
        'data: {"candidates":[{"content":{"parts":[{"text":"h"}]}}]}\n\n',
        'data: {"candidates":[{"content":{"parts":[{"text":"i"}]}}]}\n\n',
        'data: {"candidates":[{"content":{"parts":[{"text":""}],"finishReason":"STOP"}}],'
        '"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":8,"totalTokenCount":18}}\n\n',
    ],
}


class Handler(http.server.BaseHTTPRequestHandler):
    # Required for the chunked Transfer-Encoding streaming path below.
    protocol_version = "HTTP/1.1"

    def _send_chunk(self, data: bytes):
        self.wfile.write(f"{len(data):x}\r\n".encode())
        self.wfile.write(data)
        self.wfile.write(b"\r\n")
        self.wfile.flush()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        raw = self.rfile.read(length) if length else b"{}"
        try:
            req = json.loads(raw)
        except ValueError:
            req = {}

        provider = self.headers.get("X-Mock-Provider", "openai")
        if provider not in NON_STREAM:
            self.send_error(400, f"unknown provider {provider!r}")
            return

        streaming = bool(req.get("stream"))
        if streaming and provider not in STREAM_EVENTS:
            self.send_error(400, f"no streaming fixture for provider {provider!r}")
            return

        if not streaming:
            body = json.dumps(NON_STREAM[provider]).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return

        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        for event in STREAM_EVENTS[provider]:
            self._send_chunk(event.encode())
            time.sleep(0.05)  # force separate reads, closer to a real stream
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def log_message(self, fmt, *args):
        pass  # keep container logs focused on request shape, not access noise


class Server(socketserver.ThreadingMixIn, socketserver.TCPServer):
    daemon_threads = True
    allow_reuse_address = True


if __name__ == "__main__":
    with Server(("", PORT), Handler) as httpd:
        httpd.serve_forever()
