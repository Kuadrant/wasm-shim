"""A deliberately slow, generic gRPC server.

It accepts any unary gRPC call (bytes in / bytes out), sleeps for SLEEP_SECONDS,
then returns an empty response. The point is to keep a wasm-shim *guard* gRPC
call in flight long enough to span a whole response-body stream, so the filter
keeps returning Pause on each on_http_response_body callback and Envoy buffers
(accumulates) the response body.
"""

import os
import time
from concurrent import futures

import grpc

SLEEP_SECONDS = float(os.environ.get("SLEEP_SECONDS", "5"))


class SlowGenericHandler(grpc.GenericRpcHandler):
    def service(self, handler_call_details):
        def handle(request_bytes, context):
            print(
                f"slow_grpc: call {handler_call_details.method} -> sleeping {SLEEP_SECONDS}s",
                flush=True,
            )
            time.sleep(SLEEP_SECONDS)
            print(f"slow_grpc: call {handler_call_details.method} -> replying", flush=True)
            return b""

        # request_deserializer / response_serializer = None => raw bytes pass-through
        return grpc.unary_unary_rpc_method_handler(handle)


def main():
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=16))
    server.add_generic_rpc_handlers((SlowGenericHandler(),))
    server.add_insecure_port("0.0.0.0:50051")
    server.start()
    print(f"slow_grpc: listening on 0.0.0.0:50051, sleep={SLEEP_SECONDS}s", flush=True)
    server.wait_for_termination()


if __name__ == "__main__":
    main()
