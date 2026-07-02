#!/usr/bin/env bash
# Statusline badge for tt yagni mode — prints [YAGNI] (or [YAGNI:ULTRA] etc.)
# when the mode flag file is set. Append its output to an existing statusline
# command, e.g.: your-statusline.sh; bash .../yagni-statusline.sh
#
# Adapted from hooks/ponytail-statusline.sh in the "ponytail" plugin by
# Dietrich Gebert (MIT): https://github.com/DietrichGebert/ponytail
flag="$HOME/.claude/.tt-yagni-mode"
[ -f "$flag" ] || exit 0

mode=$(head -n1 "$flag" | tr -d '[:space:]')

if [ -z "$mode" ] || [ "$mode" = "full" ]; then
    printf '\033[38;5;108m[YAGNI]\033[0m'
else
    printf '\033[38;5;108m[YAGNI:%s]\033[0m' "$(printf '%s' "$mode" | tr '[:lower:]' '[:upper:]')"
fi
