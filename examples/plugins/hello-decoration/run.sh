#!/bin/sh
# Newline JSON decoration. One reply per request. See docs/plugins.md.
while IFS= read -r line; do
  case "$line" in
    *'"op":"kill"'*) printf '%s\n' '{"cancelled":true}' ;;
    *) printf '%s\n' '{"chrome":{"top":28,"bottom":0,"left":0,"right":0,"ops":[{"text":{"x":8,"y":6,"text":"hello","color":"#c0caf5"}}],"hits":[]},"ops":[]}' ;;
  esac
done
