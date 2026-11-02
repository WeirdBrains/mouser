# Mouser shell integration for zsh (docs/protocol/shell.md, Fuzzball `s`, v0).
# Load it from ~/.zshrc:   eval "$(mouser shell-integration zsh)"
# It reports the directory (OSC 7) and lets Mouser change directory silently,
# keeping the line you are typing.
[[ -o interactive && -z ${_mouser_si-} ]] || return 0
typeset -g _mouser_si=1

_mouser_pct() {
  emulate -L zsh
  local LC_ALL=C s=$1 o= c
  local -i i
  for (( i = 1; i <= ${#s}; i++ )); do
    c=${s[i]}
    case $c in
      [A-Za-z0-9._~/-]) o+=$c ;;
      *) o+=$(printf '%%%02X' "'$c") ;;
    esac
  done
  REPLY=$o
}

_mouser_osc7() {
  _mouser_pct "$PWD"
  printf '\e]7;file://%s%s\e\\' "${HOST:-}" "$REPLY"
}

# At the prompt: silent actions are welcome. While a command runs: not.
_mouser_precmd() {
  printf '\eP=Fzsa;v=0,shell=zsh,caps=cwd\e\\'
  _mouser_osc7
}
_mouser_preexec() { printf '\eP=Fzsa;v=0,caps=\e\\'; }

# DCS = F z s c ; id=N,path=PCT ST   (the bound prefix is already read)
_mouser_sc() {
  emulate -L zsh
  local c rest= id= p= kv dir
  while read -rk 1 c; do
    if [[ $c == $'\e' ]]; then read -rk 1 c; break; fi
    rest+=$c
    (( ${#rest} > 16384 )) && return 0
  done
  for kv in ${(s:,:)${rest#;}}; do
    case $kv in
      id=*) id=${kv#id=} ;;
      path=*) p=${kv#path=} ;;
    esac
  done
  [[ $id == <-> ]] || return 0
  dir=${(g::)${p//\%/\\x}}
  if [[ -n $dir ]] && builtin cd -- "$dir" 2>/dev/null; then
    printf '\eP=Fzsr;id=%s,ok=1\e\\' "$id"
  else
    _mouser_pct "cannot cd to $dir"
    printf '\eP=Fzsr;id=%s,ok=0,err=ENOENT,msg=%s\e\\' "$id" "$REPLY"
  fi
  # Refresh what the prompt shows (vcs info etc.), then redraw it in place;
  # BUFFER and CURSOR are untouched.
  local f
  for f in $precmd_functions; do "$f"; done
  zle reset-prompt
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _mouser_precmd
add-zsh-hook preexec _mouser_preexec
zle -N _mouser_sc
() {
  local m
  for m in emacs viins vicmd; do bindkey -M $m '\eP=Fzsc' _mouser_sc; done
}
