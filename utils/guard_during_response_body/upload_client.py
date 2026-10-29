"""Slow chunked request-body uploader.

Sends POST /upload to Envoy (127.0.0.1:18000) with a JSON body streamed as
several HTTP/1.1 chunks spread over a couple of seconds, with the `marker` field
LAST. While a slow guard gRPC is in flight the shim returns Pause on each
on_http_request_body callback, so Envoy accumulates the request body and the
offset-0 read in StoreTask re-reads the already-consumed prefix.
"""

import json
import socket
import time

HOST, PORT = "127.0.0.1", 18000
N_CHUNKS = 6
GAP = 0.25

body = json.dumps({"pad": "x" * 4000, "marker": "found-it"}).encode()
print(f"upload_client: body={len(body)} bytes, {N_CHUNKS} chunks, gap={GAP}s")

s = socket.create_connection((HOST, PORT))
req = (
    "POST /upload HTTP/1.1\r\n"
    "Host: emb.example.com\r\n"
    "Content-Type: application/json\r\n"
    "Transfer-Encoding: chunked\r\n"
    "\r\n"
)
s.sendall(req.encode())

size = max(1, len(body) // N_CHUNKS)
for off in range(0, len(body), size):
    part = body[off : off + size]
    s.sendall(f"{len(part):X}\r\n".encode() + part + b"\r\n")
    print(f"upload_client: sent chunk off={off} len={len(part)}")
    time.sleep(GAP)
s.sendall(b"0\r\n\r\n")

s.settimeout(30)
resp = b""
try:
    while True:
        buf = s.recv(4096)
        if not buf:
            break
        resp += buf
        if b"\r\n\r\n" in resp and len(resp) > 16:
            break
except socket.timeout:
    pass
s.close()
status = resp.split(b"\r\n", 1)[0].decode(errors="replace") if resp else "<no response>"
print(f"upload_client: response status = {status}")
