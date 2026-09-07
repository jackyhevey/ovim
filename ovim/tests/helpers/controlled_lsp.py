"""LSP peer with an explicit initialize barrier and an inspectable wire log."""

import json
import os
import pathlib
import sys
import time

root = pathlib.Path(sys.argv[1])


def respond(request, payload):
    body = json.dumps({"jsonrpc": "2.0", "id": request["id"], **payload}).encode()
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    sys.stdout.buffer.flush()


while True:
    headers = {}
    while line := sys.stdin.buffer.readline():
        if line == b"\r\n":
            break
        key, value = line.decode().split(":", 1)
        headers[key.lower()] = value.strip()
    if not headers:
        break
    request = json.loads(sys.stdin.buffer.read(int(headers["content-length"])))
    with (root / "events.jsonl").open("a") as log:
        log.write(json.dumps({**request, "peerPid": os.getpid()}) + "\n")
    method = request.get("method")
    if method == "initialize":
        while not (root / "initialize-response.json").exists():
            if not root.exists():
                sys.exit(0)
            time.sleep(0.01)
        respond(request, json.loads((root / "initialize-response.json").read_text()))
    elif method == "exit":
        break
    elif "id" in request:
        respond(request, {"result": None})
