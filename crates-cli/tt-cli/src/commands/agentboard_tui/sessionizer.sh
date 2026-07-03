#!/usr/bin/env bash
# agentboard sessionizer — fuzzy directory picker for new tmux sessions
# Requires: fzf, find

SEARCH_DIR="${SESSIONIZER_DIR:-$HOME/code}"

if ! command -v fzf &>/dev/null; then
  echo "fzf is required for the sessionizer. Install it: https://github.com/junegunn/fzf"
  exit 1
fi

if [ ! -d "$SEARCH_DIR" ]; then
  echo "Directory not found: $SEARCH_DIR"
  exit 1
fi

selected=$(find "$SEARCH_DIR" -mindepth 1 -maxdepth 3 -type d 2>/dev/null | fzf \
  --reverse \
  --header="Pick a directory for new session" \
  --preview=':' \
  --preview-window=hidden \
  --bind='ctrl-c:abort')

[ -z "$selected" ] && exit 0

# Derive session name from directory basename, replacing dots with underscores
session_name=$(basename "$selected" | tr '.' '_')

# If session already exists, just switch to it
if tmux has-session -t "=$session_name" 2>/dev/null; then
  tmux switch-client -t "$session_name"
  exit 0
fi

tmux new-session -d -s "$session_name" -c "$selected"
tmux switch-client -t "$session_name"
