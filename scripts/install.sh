#!/bin/sh
# Build and install FinderViewer without a Python runtime.
set -eu
project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
shell_kind=auto
binary_source=
usage() { echo '사용법: scripts/install.sh [--shell auto|zsh|bash|none] [--binary PATH]' >&2; }
while [ "$#" -gt 0 ]; do
  case "$1" in
    --shell) [ "$#" -ge 2 ] || { usage; exit 1; }; shell_kind=$2; shift 2 ;;
    --binary) [ "$#" -ge 2 ] || { usage; exit 1; }; binary_source=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage; exit 1 ;;
  esac
done
if [ -n "$binary_source" ] && { [ ! -f "$binary_source" ] || [ ! -x "$binary_source" ]; }; then
  echo "실행 가능한 바이너리 파일이 아닙니다: $binary_source" >&2; exit 1
fi
if [ "$shell_kind" = auto ]; then
  shell_kind=${SHELL:-}
  shell_kind=${shell_kind##*/}
  case "$shell_kind" in zsh|bash) ;; *) shell_kind=none ;; esac
fi
case "$shell_kind" in zsh|bash|none) ;; *) echo 'shell은 auto, zsh, bash, none 중 하나여야 합니다.' >&2; exit 1 ;; esac
bindir=$HOME/.local/bin
shared=$HOME/.local/share/finderviewer
destination=$bindir/finderviewer
marker=$shared/installed-sha256
checksum() { if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'; else sha256sum "$1" | awk '{print $1}'; fi; }
exists() { [ -e "$1" ] || [ -L "$1" ]; }
legacy_link() {
  [ -L "$1" ] || return 1
  link=$(readlink "$1")
  case "$link" in /*) ;; *) link=$(dirname "$1")/$link ;; esac
  if [ -d "$(dirname "$link")" ]; then
    link=$(CDPATH= cd -- "$(dirname "$link")" && printf '%s/%s\n' "$(pwd -P)" "$(basename "$link")")
  fi
  case "$link" in
    "$project/.venv/bin/finderviewer"|"$project/.venv/bin/finderview") return 0 ;;
    "$HOME/Project/finderView/terminal/.venv/bin/finderviewer"|"$HOME/Project/finderView/terminal/.venv/bin/finderview") return 0 ;;
    *) return 1 ;;
  esac
}
if exists "$destination"; then
  if legacy_link "$destination"; then :
  elif [ -f "$marker" ] && [ -f "$destination" ] && [ ! -L "$destination" ] && [ "$(checksum "$destination")" = "$(cat "$marker")" ]; then :
  else echo "기존의 다른 명령을 덮어쓰지 않습니다: $destination" >&2; exit 1; fi
fi
alias_ok=yes
if exists "$bindir/finderview"; then
  if legacy_link "$bindir/finderview"; then :
  elif [ -L "$bindir/finderview" ] && [ "$(readlink "$bindir/finderview")" = finderviewer ]; then :
  else alias_ok=no; fi
fi
commands=finderviewer
[ "$alias_ok" = no ] || commands='finderviewer finderview'
work=$(mktemp -d "${TMPDIR:-/tmp}/finderviewer-install.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
begin='# >>> FinderViewer terminal >>>'
end='# <<< FinderViewer terminal <<<'
rc=
completion=
if [ "$shell_kind" != none ]; then
  if [ "$shell_kind" = zsh ]; then
    rc=$HOME/.zshrc
    completion=$HOME/.zsh/completions/_finderviewer
    completion_source=$project/completions/_finderviewer
    cat > "$work/body" <<BODY
if [[ -o interactive ]]; then
  if [[ ":\$PATH:" != *":\$HOME/.local/bin:"* ]]; then
    export PATH="\$HOME/.local/bin:\$PATH"
  fi
  fpath=("\$HOME/.zsh/completions" \$fpath)
  autoload -Uz compinit _finderviewer
  (( \$+functions[compdef] )) || compinit
  compdef _finderviewer $commands
fi
BODY
  else
    rc=$HOME/.bashrc
    completion=$shared/finderviewer.bash
    completion_source=$project/completions/finderviewer.bash
    cat > "$work/body" <<BODY
case ":\$PATH:" in
  *":\$HOME/.local/bin:"*) ;;
  *) export PATH="\$HOME/.local/bin:\$PATH" ;;
esac
if [[ \$- == *i* ]]; then
  source "\$HOME/.local/share/finderviewer/finderviewer.bash" $commands
fi
BODY
  fi
  if exists "$completion" && ! cmp -s "$completion_source" "$completion" && ! grep -q '^# FinderViewer managed completion$' "$completion"; then
    known_legacy=$HOME/Project/finderView/terminal/completions/_finderviewer
    if [ "$shell_kind" = zsh ] && legacy_link "$destination" && [ -f "$known_legacy" ] && cmp -s "$known_legacy" "$completion"; then :
    else echo "기존의 다른 자동완성 파일을 유지했습니다: $completion" >&2; exit 1
    fi
  fi
  if [ -f "$rc" ]; then cp "$rc" "$work/original"; else : > "$work/original"; fi
  awk -v first="$begin" -v last="$end" '
    $0==first { if (inside || seen++) exit 2; inside=1; next }
    $0==last { if (!inside) exit 2; inside=0; next }
    !inside { print }
    END { if (inside) exit 2 }
  ' "$work/original" > "$work/newrc" || { echo '자동완성 관리 블록이 올바르지 않아 설치를 중단했습니다.' >&2; exit 1; }
  printf '\n%s\n' "$begin" >> "$work/newrc"
  cat "$work/body" >> "$work/newrc"
  printf '%s\n' "$end" >> "$work/newrc"
fi
if [ -z "$binary_source" ]; then
  if command -v cargo >/dev/null 2>&1; then cargo_command=cargo
  elif [ -x "$HOME/.cargo/bin/cargo" ]; then cargo_command=$HOME/.cargo/bin/cargo
  else echo 'Rust가 필요합니다. https://rustup.rs 에서 설치한 뒤 새 터미널에서 다시 실행하세요.' >&2; exit 1
  fi
  (cd "$project" && "$cargo_command" build --release --locked)
  binary_source=$project/target/release/finderviewer
fi
mkdir -p "$bindir" "$shared"
stamp=$(date +%Y%m%d-%H%M%S)-$$
if exists "$destination"; then cp -P "$destination" "$destination.backup-$stamp"; fi
binary_stage=$(mktemp "$bindir/.finderviewer.XXXXXX")
cp "$binary_source" "$binary_stage"
chmod 755 "$binary_stage"
# mv replaces the old symlink itself, leaving its target untouched.
mv -f "$binary_stage" "$destination"
checksum "$destination" > "$work/checksum"
cp "$work/checksum" "$marker"
if [ "$alias_ok" = yes ]; then
  if exists "$bindir/finderview" && legacy_link "$bindir/finderview"; then
    cp -P "$bindir/finderview" "$bindir/finderview.backup-$stamp"
    rm "$bindir/finderview"
  fi
  if ! exists "$bindir/finderview"; then ln -s finderviewer "$bindir/finderview"; fi
else
  echo 'finderview 별칭은 다른 명령과 겹쳐 유지했습니다. finderviewer를 사용하세요.'
fi
if [ -n "$rc" ]; then
  mkdir -p "$(dirname "$completion")"
  if exists "$completion"; then cp -P "$completion" "$completion.backup-$stamp"; fi
  completion_stage=$(mktemp "$(dirname "$completion")/.finderviewer-completion.XXXXXX")
  cp "$completion_source" "$completion_stage"
  chmod 644 "$completion_stage"
  mv -f "$completion_stage" "$completion"
  # Preserve symlinked shell configuration files by writing their resolved target.
  target=$rc
  depth=0
  while [ -L "$target" ]; do
    depth=$((depth + 1))
    [ "$depth" -le 40 ] || { echo '셸 설정 심볼릭 링크가 너무 깊습니다.' >&2; exit 1; }
    link=$(readlink "$target")
    case "$link" in /*) target=$link ;; *) target=$(dirname "$target")/$link ;; esac
  done
  mkdir -p "$(dirname "$target")"
  if [ -f "$target" ]; then cp -p "$target" "$rc.finderviewer-backup-$stamp"; fi
  rc_stage=$(mktemp "$(dirname "$target")/.finderviewer-rc.XXXXXX")
  if [ -f "$target" ]; then cp -p "$target" "$rc_stage"; else chmod 644 "$rc_stage"; fi
  cat "$work/newrc" > "$rc_stage"
  mv -f "$rc_stage" "$target"
fi
printf '설치 완료: %s\n' "$destination"
if [ "$shell_kind" = none ]; then
  echo '~/.local/bin을 PATH에 추가하거나 ~/.local/bin/finderviewer . 으로 실행하세요.'
else
  printf '새 %s 터미널에서 finderviewer . 을 실행하세요. 폴더 Tab 자동완성이 등록됐습니다.\n' "$shell_kind"
fi
