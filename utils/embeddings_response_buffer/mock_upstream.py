#!/usr/bin/env python3
"""Mock OpenAI-style embeddings upstream.

Returns a single `application/json` embeddings response whose `usage` object is
*trailing* -- after a large `data` array of float vectors -- just like the real
OpenAI `/v1/embeddings` schema. The body is sized well above Envoy's 1 MiB
per-connection buffer limit so that any filter which buffers the *whole* body to
reach the trailing `/usage/total_tokens` will trip `response_payload_too_large`.

Responds the same way to any method/path (the gateway routes `/v1/embeddings`
here), so a POST works without the 405 a static-file server would return.
"""
import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer

N_INPUTS = int(os.environ.get("N_INPUTS", "400"))  # ~2.7 MiB body (> 1 MiB)
DIM = int(os.environ.get("DIM", "768"))
TOTAL_TOKENS = int(os.environ.get("TOTAL_TOKENS", "42"))

_vector = [round(0.1234567 + (i % 10) * 0.01, 7) for i in range(DIM)]
_data = [{"object": "embedding", "index": i, "embedding": _vector} for i in range(N_INPUTS)]
# `usage` is serialized LAST, after the big `data` array -- the whole point.
BODY = json.dumps(
    {
        "object": "list",
        "data": _data,
        "model": "granite-embedding",
        "usage": {"prompt_tokens": 20, "total_tokens": TOTAL_TOKENS},
    }
).encode()


class Handler(BaseHTTPRequestHandler):
    def _respond(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)

    def do_POST(self):
        self._respond()

    def do_GET(self):
        self._respond()

    def log_message(self, fmt, *args):
        pass


if __name__ == "__main__":
    print(f"mock upstream: body={len(BODY)} bytes, inputs={N_INPUTS}, dim={DIM}", flush=True)
    HTTPServer(("0.0.0.0", 80), Handler).serve_forever()
