# bm-complete ZLE integration shim
# Connects the bm-complete daemon to Zsh's completion system via skim
#
# When tab is pressed, this widget:
# 1. Sends the current BUFFER to the daemon via Unix socket
# 2. Receives completion candidates as JSON
# 3. Pipes through skim for interactive selection
# 4. Inserts the selected completion
#
# Requires: bm-complete daemon running, sk (skim) on PATH

: ${BM_COMPLETE_SOCKET:=/tmp/bm-complete.socket}
: ${BM_COMPLETE_BIN:=$(command -v bm-complete 2>/dev/null)}

# Lazy daemon start: start daemon if socket doesn't exist
_bm_complete_ensure_daemon() {
  if [[ ! -S "$BM_COMPLETE_SOCKET" ]] && [[ -n "$BM_COMPLETE_BIN" ]]; then
    "$BM_COMPLETE_BIN" daemon --socket "$BM_COMPLETE_SOCKET" &>/dev/null &
    disown
    sleep 0.1 # brief wait for socket to appear
  fi
}

_bm_complete_widget() {
  _bm_complete_ensure_daemon

  # Fall back to default completion if daemon unavailable
  if [[ ! -S "$BM_COMPLETE_SOCKET" ]]; then
    zle expand-or-complete
    return
  fi

  # Query daemon
  local request='{"buffer":"'"${BUFFER//\"/\\\"}"'","position":'${CURSOR}'}'
  local response
  response=$(echo "$request" | socat - UNIX-CONNECT:"$BM_COMPLETE_SOCKET" 2>/dev/null)

  if [[ -z "$response" ]] || [[ "$response" == *'"error"'* ]]; then
    zle expand-or-complete
    return
  fi

  # Extract completions and pipe through skim
  local selected
  selected=$(echo "$response" | jaq -r '.[] | .completion + "\t" + .description' 2>/dev/null |
    sk --height=40% --layout=reverse --delimiter='\t' \
       --preview-window=hidden \
       --header='Tab completions' |
    cut -f1)

  if [[ -n "$selected" ]]; then
    # Replace the current word with the selection
    local words=("${(z)BUFFER}")
    local last_word="${words[-1]}"
    local prefix="${BUFFER%$last_word}"
    BUFFER="${prefix}${selected}"
    CURSOR=${#BUFFER}
  fi

  zle reset-prompt
}

zle -N _bm_complete_widget
bindkey '^I' _bm_complete_widget # Tab key
