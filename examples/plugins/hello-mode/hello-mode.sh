#!/bin/sh
# Mouser sends {"api":1,"command":"mode","args":[EVENT,ID],"cwd":"..."} on stdin
# (EVENT: activate, key or cwd; ID: the [[mode.keys]] id) and reads
# {"title":"...","lines":["..."],"text":"status"} on stdout.
read -r req
event=$(printf '%s' "$req" | sed -n 's/.*"args":\["\([^"]*\)".*/\1/p')
id=$(printf '%s' "$req" | sed -n 's/.*"args":\["[^"]*","\([^"]*\)".*/\1/p')
cwd=$(printf '%s' "$req" | sed -n 's/.*"cwd":"\([^"]*\)".*/\1/p')
where=${cwd:-"(not shared: allow files.read)"}
case "$id" in
  time) extra="it is $(date +%H:%M:%S)" ;;
  files) if [ -n "$cwd" ]; then extra="$(ls -A "$cwd" 2>/dev/null | wc -l | tr -d ' ') entries here"; else extra="no directory shared"; fi ;;
  *) extra="press t for the time, f to count files" ;;
esac
printf '{"title":"hello from %s","lines":["event: %s","in: %s","%s"],"text":"hello-mode: %s"}\n' \
  "$MOUSER_PLUGIN_NAME" "$event" "$where" "$extra" "$event"
