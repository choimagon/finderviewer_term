"""Installer tests use an isolated HOME and a fake checkout, never real rc files."""
from __future__ import annotations

import importlib.util
import os
from pathlib import Path
from uuid import uuid4

import pytest


@pytest.fixture
def installation(tmp_path: Path, monkeypatch):
    script = Path(__file__).resolve().parents[1] / "scripts" / "install.py"
    spec = importlib.util.spec_from_file_location(f"finderview_installer_{uuid4().hex}", script)
    assert spec is not None and spec.loader is not None
    installer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(installer)
    project = tmp_path / "checkout with spaces"
    home = tmp_path / "isolated home"
    home.mkdir()
    (project / ".venv" / "bin").mkdir(parents=True)
    (project / "completions").mkdir()
    for name in ("finderviewer", "finderview"):
        executable = project / ".venv" / "bin" / name
        executable.write_text("#!/bin/sh\nexit 0\n")
        executable.chmod(0o755)
    (project / "completions" / "_finderviewer").write_text("#compdef finderviewer finderview\n_arguments '*:directory:_directories'\n")
    (project / "completions" / "finderviewer.bash").write_text("complete -o filenames -F _finderviewer finderviewer finderview\n")
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("SHELL", "/bin/zsh")
    monkeypatch.setenv("PATH", "/usr/bin:/bin")
    monkeypatch.setattr(installer, "PROJECT", project)
    return installer, project, home


def assert_command(home: Path, project: Path, name: str):
    command = home / ".local" / "bin" / name
    assert command.is_symlink()
    assert command.resolve() == (project / ".venv" / "bin" / name).resolve()


def test_zsh_preserves_existing_configuration_and_backs_it_up(installation):
    installer, project, home = installation
    original = "# user configuration\nexport KEEP_EXISTING=1\nalias ll='ls -la'\n"
    config = home / ".zshrc"
    config.write_text(original)
    installer.install(shell="zsh")
    assert_command(home, project, "finderviewer")
    assert_command(home, project, "finderview")
    updated = config.read_text()
    assert original.rstrip() in updated
    assert updated.count(installer.BEGIN) == 1 and updated.count(installer.END) == 1
    assert ".local/bin" in updated and ".zsh/completions" in updated
    assert "compinit" in updated and "_finderviewer" in updated
    assert (home / ".zsh" / "completions" / "_finderviewer").read_bytes() == (project / "completions" / "_finderviewer").read_bytes()
    backups = list(home.glob(".zshrc.finderviewer-backup-*"))
    assert len(backups) == 1 and backups[0].read_text() == original
    assert not (home / ".bashrc").exists()


def test_idempotent_zsh_reinstall_does_not_duplicate_block_or_backup(installation):
    installer, project, home = installation
    (home / ".zshrc").write_text("export ORIGINAL=1\n")
    installer.install(shell="zsh")
    before = (home / ".zshrc").read_bytes()
    backups = list(home.glob(".zshrc.finderviewer-backup-*"))
    installer.install(shell="zsh")
    assert (home / ".zshrc").read_bytes() == before
    assert list(home.glob(".zshrc.finderviewer-backup-*")) == backups
    assert before.decode().count(installer.BEGIN) == 1
    assert_command(home, project, "finderviewer")


def test_bash_install_has_completion_and_preserves_zsh(installation):
    installer, project, home = installation
    bash_original = "# bash user settings\nexport BASH_KEEP=1\n"
    zsh_original = "export ZSH_KEEP=1\n"
    (home / ".bashrc").write_text(bash_original)
    (home / ".zshrc").write_text(zsh_original)
    installer.install(shell="bash")
    assert_command(home, project, "finderviewer")
    updated = (home / ".bashrc").read_text()
    assert bash_original.rstrip() in updated
    assert updated.count(installer.BEGIN) == 1
    assert ".local/share/finderviewer/finderviewer.bash" in updated
    assert (home / ".local" / "share" / "finderviewer" / "finderviewer.bash").read_bytes() == (project / "completions" / "finderviewer.bash").read_bytes()
    assert (home / ".zshrc").read_text() == zsh_original
    backups = list(home.glob(".bashrc.finderviewer-backup-*"))
    assert len(backups) == 1 and backups[0].read_text() == bash_original
    installer.install(shell="bash")
    assert (home / ".bashrc").read_text() == updated
    assert len(list(home.glob(".bashrc.finderviewer-backup-*"))) == 1


def test_shell_none_only_installs_commands(installation):
    installer, project, home = installation
    for name in (".zshrc", ".bashrc"):
        (home / name).write_text("original\n")
    installer.install(shell="none")
    assert_command(home, project, "finderviewer")
    assert_command(home, project, "finderview")
    for name in (".zshrc", ".bashrc"):
        assert (home / name).read_text() == "original\n"
    assert not (home / ".zsh").exists()
    assert not (home / ".local" / "share" / "finderviewer").exists()
    assert not list(home.glob(".*rc.finderviewer-backup-*"))


def test_unrelated_primary_command_aborts_without_modifying_configuration(installation):
    installer, project, home = installation
    bindir = home / ".local" / "bin"
    bindir.mkdir(parents=True)
    existing = bindir / "finderviewer"
    existing.write_bytes(b"unrelated existing command")
    original = "export DO_NOT_TOUCH=1\n"
    (home / ".zshrc").write_text(original)
    with pytest.raises(RuntimeError):
        installer.install(shell="zsh")
    assert existing.read_bytes() == b"unrelated existing command"
    assert not existing.is_symlink()
    assert (home / ".zshrc").read_text() == original
    assert not list(home.glob(".zshrc.finderviewer-backup-*"))
    assert not (home / ".zsh" / "completions" / "_finderviewer").exists()


def test_unrelated_optional_alias_is_preserved_without_blocking_primary(installation):
    installer, project, home = installation
    bindir = home / ".local" / "bin"
    bindir.mkdir(parents=True)
    alias = bindir / "finderview"
    alias.write_bytes(b"unrelated finderview alias")
    installer.install(shell="zsh")
    assert_command(home, project, "finderviewer")
    assert alias.read_bytes() == b"unrelated finderview alias" and not alias.is_symlink()
    assert installer.BEGIN in (home / ".zshrc").read_text()


@pytest.mark.parametrize("shell,relative", [("zsh", ".zsh/completions/_finderviewer"), ("bash", ".local/share/finderviewer/finderviewer.bash")])
def test_unrelated_completion_aborts_before_changing_shell_configuration(installation, shell, relative):
    installer, project, home = installation
    completion = home / relative
    completion.parent.mkdir(parents=True)
    completion.write_bytes(b"other completion owner")
    config = home / (".zshrc" if shell == "zsh" else ".bashrc")
    config.write_text("# preserve\n")
    with pytest.raises(RuntimeError):
        installer.install(shell=shell)
    assert completion.read_bytes() == b"other completion owner"
    assert config.read_text() == "# preserve\n"
    assert not list(home.glob(".*rc.finderviewer-backup-*"))
    assert not os.path.lexists(home / ".local" / "bin" / "finderviewer")


@pytest.mark.parametrize("shell_executable,config", [("/bin/zsh", ".zshrc"), ("/usr/local/bin/bash", ".bashrc")])
def test_auto_shell_uses_shell_environment(installation, monkeypatch, shell_executable, config):
    installer, project, home = installation
    monkeypatch.setenv("SHELL", shell_executable)
    installer.install(shell="auto")
    assert_command(home, project, "finderviewer")
    assert installer.BEGIN in (home / config).read_text()


def test_missing_virtualenv_does_not_touch_shell_configuration(installation):
    installer, project, home = installation
    (project / ".venv" / "bin" / "finderviewer").unlink()
    original = "# existing user config\n"
    (home / ".zshrc").write_text(original)
    with pytest.raises((RuntimeError, FileNotFoundError)):
        installer.install(shell="zsh")
    assert (home / ".zshrc").read_text() == original
    assert not list(home.glob(".zshrc.finderviewer-backup-*"))
    assert not os.path.lexists(home / ".local" / "bin" / "finderviewer")
