# FinderViewer managed completion
# source this-file [finderviewer finderview]
_finderviewer_complete() {
  local current="${COMP_WORDS[COMP_CWORD]}"
  COMPREPLY=()
  while IFS= read -r directory; do
    COMPREPLY+=("${directory%/}/")
  done < <(compgen -d -- "$current")
  compopt -o filenames -o nospace
}
if (( $# )); then
  complete -F _finderviewer_complete "$@"
else
  complete -F _finderviewer_complete finderviewer
fi
