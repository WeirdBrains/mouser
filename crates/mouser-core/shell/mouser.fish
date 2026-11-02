# Mouser shell integration for fish 3.4+ (docs/protocol/shell.md, Fuzzball `s`, v0).
# Load it from ~/.config/fish/config.fish:   mouser shell-integration fish | source
# It reports the directory (OSC 7) and lets Mouser change directory silently,
# keeping the line you are typing.
status is-interactive; or exit 0
set -q __mouser_si; and exit 0
set -g __mouser_si 1

function __mouser_pct
    string escape --style=url -- $argv[1]
end

function __mouser_osc7 --on-variable PWD
    printf '\e]7;file://%s%s\e\\' (hostname) (__mouser_pct $PWD)
end

function __mouser_prompt --on-event fish_prompt
    printf '\eP=Fzsa;v=0,shell=fish,caps=cwd\e\\'
    __mouser_osc7
end

function __mouser_preexec --on-event fish_preexec
    printf '\eP=Fzsa;v=0,caps=\e\\'
end

# DCS = F z s c ; id=N,path=PCT ST   (the bound prefix is already read)
function __mouser_sc
    set -l rest ''
    while true
        set -l c (dd bs=1 count=1 2>/dev/null </dev/tty | string collect)
        test -z "$c" -o "$c" = \e; and break
        set rest "$rest$c"
        test (string length -- $rest) -gt 16384; and return
    end
    dd bs=1 count=1 2>/dev/null </dev/tty >/dev/null
    set -l id (string match -r 'id=([0-9]+)' -- $rest)[2]
    set -l p (string match -r 'path=([^,]*)' -- $rest)[2]
    test -n "$id"; or return
    set -l dir (string unescape --style=url -- $p)
    if test -n "$dir"; and builtin cd -- $dir 2>/dev/null
        printf '\eP=Fzsr;id=%s,ok=1\e\\' $id
    else
        printf '\eP=Fzsr;id=%s,ok=0,err=ENOENT,msg=%s\e\\' $id (__mouser_pct "cannot cd to $dir")
    end
    commandline -f repaint
end

for m in default insert
    bind -M $m \eP=Fzsc __mouser_sc 2>/dev/null
end
