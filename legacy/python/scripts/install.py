"""Register this checkout's virtualenv commands and optional shell completion."""
from __future__ import annotations

import argparse
from datetime import datetime
import os
from pathlib import Path
import shutil
import sys
import tempfile

PROJECT = Path(__file__).resolve().parents[1]
BEGIN = "# >>> FinderViewer terminal >>>"
END = "# <<< FinderViewer terminal <<<"
MANAGED = "# FinderViewer managed completion"


def _owned_link(destination: Path, source: Path) -> bool:
    return destination.is_symlink() and destination.resolve() == source.resolve()


def _configuration(original: str, block: str) -> str:
    if BEGIN in original or END in original:
        if original.count(BEGIN) != 1 or original.count(END) != 1:
            raise RuntimeError("자동완성 관리 블록이 올바르지 않습니다. 기존 설정을 유지했습니다.")
        start = original.index(BEGIN)
        stop = original.index(END) + len(END)
        if stop < start:
            raise RuntimeError("자동완성 관리 블록 순서가 올바르지 않습니다.")
        return original[:start] + block.rstrip() + original[stop:]
    return original.rstrip() + "\n\n" + block


def _write_configuration(path: Path, original: str, updated: str) -> None:
    if original == updated:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        stamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
        backup = path.with_name(f"{path.name}.finderviewer-backup-{stamp}")
        shutil.copy2(path, backup)
        print(f"기존 셸 설정 백업: {backup}")
    # Preserve a user's dotfile symlink and replace its contents atomically.
    target = path.resolve() if path.is_symlink() else path
    mode = target.stat().st_mode & 0o777 if target.exists() else 0o644
    temporary = None
    try:
        with tempfile.NamedTemporaryFile("w", encoding="utf8", dir=target.parent, delete=False) as stream:
            temporary = Path(stream.name)
            stream.write(updated)
        temporary.chmod(mode)
        temporary.replace(target)
    finally:
        if temporary and temporary.exists():
            temporary.unlink()


def install(shell: str = "auto") -> None:
    if os.name == "nt":
        raise RuntimeError("Windows에서는 .venv\\Scripts\\finderviewer.exe로 실행하세요. 셸 자동 등록은 macOS/Linux용입니다.")
    if shell == "auto":
        detected = Path(os.environ.get("SHELL", "zsh")).name
        shell = detected if detected in ("zsh", "bash") else "none"
    if shell not in ("zsh", "bash", "none"):
        raise ValueError("shell은 auto, zsh, bash, none 중 하나여야 합니다.")
    home = Path.home()
    bindir = home / ".local" / "bin"
    primary = PROJECT / ".venv" / "bin" / "finderviewer"
    destination = bindir / "finderviewer"
    if not primary.is_file():
        raise RuntimeError("먼저 .venv/bin/python -m pip install . 을 실행하세요.")
    if os.path.lexists(destination) and not _owned_link(destination, primary):
        raise RuntimeError(f"동일한 이름의 다른 명령이 있어 덮어쓰지 않았습니다: {destination}")

    commands = ["finderviewer"]
    alias = PROJECT / ".venv" / "bin" / "finderview"
    alias_destination = bindir / "finderview"
    alias_available = alias.is_file() and (not os.path.lexists(alias_destination) or _owned_link(alias_destination, alias))
    if alias_available:
        commands.append("finderview")

    completion = None
    completion_source = None
    rc = None
    original = updated = ""
    if shell != "none":
        if shell == "zsh":
            completion_source = PROJECT / "completions" / "_finderviewer"
            completion = home / ".zsh" / "completions" / "_finderviewer"
            body = '''if [[ -o interactive ]]; then
  if [[ ":$PATH:" != *":$HOME/.local/bin:"* ]]; then
    export PATH="$HOME/.local/bin:$PATH"
  fi
  fpath=("$HOME/.zsh/completions" $fpath)
  autoload -Uz compinit _finderviewer
  (( $+functions[compdef] )) || compinit
  compdef _finderviewer COMMANDS
fi'''.replace("COMMANDS", " ".join(commands))
            rc = home / ".zshrc"
        else:
            completion_source = PROJECT / "completions" / "finderviewer.bash"
            completion = home / ".local" / "share" / "finderviewer" / "finderviewer.bash"
            body = '''case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) export PATH="$HOME/.local/bin:$PATH" ;;
esac
if [[ $- == *i* ]]; then
  source "$HOME/.local/share/finderviewer/finderviewer.bash" COMMANDS
fi'''.replace("COMMANDS", " ".join(commands))
            rc = home / ".bashrc"
        content = completion_source.read_text(encoding="utf8")
        if completion.exists():
            existing = completion.read_text(encoding="utf8")
            if existing != content and MANAGED not in existing:
                raise RuntimeError(f"기존의 다른 자동완성 파일을 유지했습니다: {completion}")
        original = rc.read_text(encoding="utf8") if rc.exists() else ""
        updated = _configuration(original, f"{BEGIN}\n{body}\n{END}\n")

    # All collision and configuration checks precede changes to the user's home.
    bindir.mkdir(parents=True, exist_ok=True)
    if not os.path.lexists(destination):
        destination.symlink_to(primary)
    if alias_available and not os.path.lexists(alias_destination):
        alias_destination.symlink_to(alias)
    elif alias.is_file() and not alias_available:
        print("finderview 별칭은 기존 명령과 겹쳐 등록하지 않았습니다. finderviewer를 사용하세요.")
    if completion and completion_source and rc:
        completion.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(completion_source, completion)
        _write_configuration(rc, original, updated)
    print(f"설치 완료: {destination}")
    if shell == "none":
        print("셸 설정은 변경하지 않았습니다. ~/.local/bin을 PATH에 추가하면 finderviewer로 실행할 수 있습니다.")
    else:
        print(f"새 {shell} 세션에서 finderviewer . 과 폴더 Tab 자동완성을 사용할 수 있습니다.")


def main() -> None:
    parser = argparse.ArgumentParser(description="finderviewer 명령과 폴더 자동완성을 등록합니다.")
    parser.add_argument("--shell", choices=("auto", "zsh", "bash", "none"), default="auto", help="자동완성을 등록할 셸. none은 명령만 연결")
    args = parser.parse_args()
    try:
        install(args.shell)
    except (OSError, RuntimeError, ValueError) as error:
        parser.exit(1, f"설치 중단: {error}\n")


if __name__ == "__main__":
    main()
