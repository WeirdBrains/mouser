# Mouser shell integration for bash 4.4+ (docs/protocol/shell.md, Fuzzball `s`, v0).
# Load it from ~/.bashrc:   eval "$(mouser shell-integration bash)"
# It reports the directory (OSC 7) and lets Mouser change directory silently,
# keeping the line you are typing.
[[ $- == *i* && -z ${__mouser_si-} ]] || return 0
__mouser_si=1

__mouser_pct() {
  local LC_ALL=C s=$1 o= c i
  for (( i = 0; i < ${#s}; i++ )); do
    c=${s:i:1}
    case $c in
      [A-Za-z0-9._~/-]) o+=$c ;;
      *) printf -v c '%%%02X' "'$c"; o+=$c ;;
    esac
  done
  REPLY=$o
}

__mouser_osc7() {
  __mouser_pct "$PWD"
  printf '\e]7;file://%s%s\e\\' "${HOSTNAME:-}" "$REPLY"
}

__mouser_precmd() {
  if [[ -n ${__mouser_redraw-} ]]; then
    # The empty line accepted after a silent cd: draw the new prompt over the old one.
    __mouser_redraw=
    printf '\e[A\r\e[J'
  fi
  printf '\eP=Fzsa;v=0,shell=bash,caps=cwd\e\\'
  __mouser_osc7
}

# DCS = F z s c ; id=N,path=PCT ST   (the bound prefix is already read)
__mouser_sc() {
  local rest c id= p= kv dir
  IFS= read -r -d $'\e' rest
  IFS= read -r -n 1 c
  local IFS=,
  for kv in ${rest#;}; do
    case $kv in
      id=*) id=${kv#id=} ;;
      path=*) p=${kv#path=} ;;
    esac
  done
  [[ $id =~ ^[0-9]+$ ]] || return 0
  printf -v dir '%b' "${p//%/\\x}"
  if [[ -n $dir ]] && builtin cd -- "$dir" 2>/dev/null; then
    printf '\eP=Fzsr;id=%s,ok=1\e\\' "$id"
  else
    __mouser_pct "cannot cd to $dir"
    printf '\eP=Fzsr;id=%s,ok=0,err=ENOENT,msg=%s\e\\' "$id" "$REPLY"
  fi
  # Bash keeps the prompt it expanded, so the macro below accepts an empty
  # line (a fresh prompt for the new directory) and then puts the line back.
  __mouser_line=$READLINE_LINE __mouser_point=$READLINE_POINT __mouser_redraw=1
  READLINE_LINE= READLINE_POINT=0
}

__mouser_restore() {
  READLINE_LINE=$__mouser_line READLINE_POINT=$__mouser_point
  __mouser_line= __mouser_point=
}

# PS0 is printed when a command starts: no silent actions until the next prompt.
PS0+=$'\eP=Fzsa;v=0,caps=\e\\'
if [[ $(declare -p PROMPT_COMMAND 2>/dev/null) == "declare -a"* ]]; then
  PROMPT_COMMAND+=(__mouser_precmd)
else
  PROMPT_COMMAND="__mouser_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
fi
bind -x '"\C-x\C-_\C-a": __mouser_sc'
bind '"\C-x\C-_\C-b": accept-line'
bind -x '"\C-x\C-_\C-c": __mouser_restore'
bind '"\eP=Fzsc": "\C-x\C-_\C-a\C-x\C-_\C-b\C-x\C-_\C-c"'
