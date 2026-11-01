#!/usr/bin/env python3
"""A minimal MCP server over stdio (newline-delimited JSON-RPC 2.0), stdlib only."""
import json
import sys

TOOLS = [{
    "name": "greet",
    "description": "Say hello to someone",
    "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}},
}]


def reply(id_, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id_}
    if error:
        msg["error"] = {"code": -32601, "message": error}
    else:
        msg["result"] = result
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    try:
        req = json.loads(line)
    except ValueError:
        continue
    method, id_ = req.get("method"), req.get("id")
    if id_ is None:
        continue  # a notification
    if method == "initialize":
        reply(id_, {"protocolVersion": req["params"].get("protocolVersion", "2025-06-18"),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "hello-mcp", "version": "0.1.0"}})
    elif method == "tools/list":
        reply(id_, {"tools": TOOLS})
    elif method == "tools/call":
        who = req["params"].get("arguments", {}).get("name", "world")
        reply(id_, {"content": [{"type": "text", "text": f"Hello, {who}!"}]})
    else:
        reply(id_, error=f"unknown method {method}")
