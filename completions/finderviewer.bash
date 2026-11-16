# FinderViewer managed completion
# source this-file [finderviewer finderview]
_finderviewer_complete() {
  local current="${COMP_WORDS[COMP_CWORD]}" index ssh_mode=no
  COMPREPLY=()
  for (( index=1; index<COMP_CWORD; index++ )); do
    if [[ ${COMP_WORDS[index]} == --ssh ]]; then ssh_mode=yes; fi
  done
  if [[ $current == --* ]]; then
    COMPREPLY=( $(compgen -W '--help --version --ssh' -- "$current") )
    return
  fi
  # Remote paths must never be completed with unrelated local directories.
  # Enter user@host or an SSH config alias explicitly. Completion does not connect.
  [[ $ssh_mode == no ]] || return
  while IFS= read -r directory; do
    COMPREPLY+=("${directory%/}/")
  done < <(compgen -d -- "$current")
  compopt -o filenames -o nospace
}
if (( $# )); then
  complete -F _finderviewer_complete "$@"
else
  complete -F _finderviewer_complete finderviewer finderview
fi
