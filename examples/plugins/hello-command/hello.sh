#!/bin/sh
# Mouser sends one JSON request on stdin: {"api":1,"command":"hello","args":[...],"cwd":"..."}
# and reads one JSON reply on stdout: {"text":"..."} or {"error":"..."}.
read -r req
args=$(printf '%s' "$req" | sed -n 's/.*"args":\[\([^]]*\)\].*/\1/p' | tr -d '"' | tr ',' ' ')
printf '{"text":"Hello, %s! (from %s)"}\n' "${args:-world}" "$MOUSER_PLUGIN_NAME"
