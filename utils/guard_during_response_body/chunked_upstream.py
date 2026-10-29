"""Mock upstream that streams a small JSON embeddings-shaped body in several
flushed HTTP/1.1 chunks, with `usage` (total_tokens) serialized LAST.

Small total (well under Envoy's 1 MiB per-connection buffer) and delivered as
multiple chunks with small gaps, so the wasm filter sees several
on_http_response_body callbacks -- the condition needed to observe whether the
body is drained (Continue) or accumulated (Pause) across callbacks.
"""

import json
import os
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

N_INPUTS = int(os.environ.get("N_INPUTS", "40"))
DIM = int(os.environ.get("DIM", "64"))
TOTAL_TOKENS = int(os.environ.get("TOTAL_TOKENS", "42"))
N_CHUNKS = int(os.environ.get("N_CHUNKS", "6"))
CHUNK_GAP = float(os.environ.get("CHUNK_GAP", "0.08"))


def build_body() -> bytes:
    vec = [round(0.1 + (i % 9) / 10.0, 7) for i in range(DIM)]
    data = [
        {"object": "embedding", "index": i, "embedding": vec}
        for i in range(N_INPUTS)
    ]
    payload = {
        "object": "list",
        "data": data,
        "model": "mock-embedding",
        "usage": {"prompt_tokens": 20, "total_tokens": TOTAL_TOKENS},
    }
    return json.dumps(payload).encode()


BODY = build_body()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _respond(self):
        print(
            f"chunked_upstream: streaming body={len(BODY)} bytes in {N_CHUNKS} chunks",
            flush=True,
        )
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        size = max(1, len(BODY) // N_CHUNKS)
        for off in range(0, len(BODY), size):
            part = BODY[off : off + size]
            self.wfile.write(f"{len(part):X}\r\n".encode())
            self.wfile.write(part)
            self.wfile.write(b"\r\n")
            self.wfile.flush()
            time.sleep(CHUNK_GAP)
        self.wfile.write(b"0\r\n\r\n")
        self.wfile.flush()

    def do_GET(self):
        self._respond()

    def do_POST(self):
        _ = self.rfile.read(int(self.headers.get("Content-Length", 0) or 0))
        self._respond()

    def log_message(self, *args):
        pass


if __name__ == "__main__":
    ThreadingHTTPServer(("0.0.0.0", 80), Handler).serve_forever()
