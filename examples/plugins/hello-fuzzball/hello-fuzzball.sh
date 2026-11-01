#!/bin/sh
# Fuzzball `p h` handshake: ask Mouser to accept this program as a plugin with
# the `notify` and `screen.read` capabilities. Mouser shows its own approval
# popup; the answer comes back on stdin as  ESC P = F z p r ; id=1,ok=1,granted=... ESC \
old=$(stty -g)
trap 'stty "$old"' EXIT INT TERM
stty raw -echo min 0 time 250        # each read waits up to 25 s (255 max)
printf '\033P=Fzph;id=1,name=hello-fuzzball,kind=tool-provider,caps=notify+screen.read\033\\'
reply=""
idle=0
while [ $idle -lt 5 ]; do                # one byte at a time until ESC \, give up after ~2 min
  c=$(dd bs=1 count=1 2>/dev/null)
  if [ -z "$c" ]; then idle=$((idle + 1)); continue; fi
  reply="$reply$c"
  case "$reply" in *'\') break ;; esac
done
reply=$(printf '%s' "$reply" | tr -d '\033' | sed -n 's/.*P=Fzpr;\(.*\)\\$/\1/p')
stty "$old"
case "$reply" in
  *ok=1*) echo "granted: ${reply#*granted=}" ;;
  "") echo "no answer (not Mouser, or timed out)" ;;
  *) echo "denied: $reply" ;;
esac
