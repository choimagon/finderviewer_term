"""Filesystem regression tests; every mutation stays in pytest temp folders."""
from __future__ import annotations

import os
from pathlib import Path
import shutil
from uuid import uuid4

import pytest

import finderview_tui.fs as filesystem
from finderview_tui.fs import BoundaryError, ConflictError, FileManager, PartialOperationError


@pytest.fixture
def workspace(tmp_path: Path):
    root = tmp_path / "한글 워크스페이스 #"
    root.mkdir()
    return FileManager(root)


@pytest.fixture
def trash_spy(tmp_path: Path, monkeypatch):
    trash = tmp_path / "mock OS Trash"
    trash.mkdir()
    records: list[tuple[Path, Path]] = []

    def send(path: str) -> None:
        source = Path(path)
        destination = trash / f"{uuid4().hex}-{source.name}"
        shutil.move(str(source), destination)
        records.append((source, destination))

    monkeypatch.setattr(filesystem, "send2trash", send)
    return records


def file(path: Path, content: bytes = b"unchanged\0\xff") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    return path


def directory(path: Path) -> Path:
    path.mkdir(parents=True, exist_ok=True)
    return path


def test_listing_all_file_types_unicode_hidden_and_sorting(workspace):
    root = workspace.root
    folder = directory(root / "중첩 폴더")
    names = ["수정본.docx", "검토.pdf", "표.xlsx", "발표.pptx", "README.md", "분석.py", "[bold] # %.txt", ".hidden"]
    for name in names:
        file(root / name)
    entries = workspace.list_dir(root)
    assert {entry.name for entry in entries} == set(names) | {folder.name}
    assert entries[0].is_dir and entries[0].path == folder
    assert not any(entry.name == ".hidden" for entry in workspace.list_dir(root, show_hidden=False))
    by_name = workspace.list_dir(root, sort="name")
    assert [entry.name for entry in by_name] == sorted(set(names) | {folder.name}, key=lambda name: (name.casefold(), name))
    with pytest.raises(ValueError):
        workspace.list_dir(root, sort="invalid")


def test_recent_sort_uses_filesystem_mtime_and_handles_empty_directory(workspace):
    root = workspace.root
    old = file(root / "a-old.txt")
    recent = file(root / "z-new.txt")
    middle = directory(root / "middle")
    for target, timestamp in [(old, 1000000000), (middle, 1100000000), (recent, 1200000000)]:
        os.utime(target, (timestamp, timestamp))
    assert [entry.path for entry in workspace.list_dir(root, sort="recent")] == [recent, middle, old]
    assert workspace.list_dir(middle) == []
    assert workspace.list_dir(root)[0].path == middle


def test_navigation_boundaries_missing_paths_and_external_symlinks(workspace, tmp_path):
    outside = directory(tmp_path / "outside")
    link = workspace.root / "외부 폴더 링크"
    link.symlink_to(outside, target_is_directory=True)
    assert any(entry.path == link and entry.is_symlink for entry in workspace.list_dir(workspace.root))
    with pytest.raises(BoundaryError):
        workspace.validate(outside)
    with pytest.raises(BoundaryError):
        workspace.validate(link, directory=True)
    with pytest.raises(BoundaryError):
        workspace.list_dir(link)
    with pytest.raises(FileNotFoundError):
        workspace.validate(workspace.root / "missing")
    with pytest.raises(NotADirectoryError):
        workspace.validate(file(workspace.root / "file.txt"), directory=True)


def test_binary_copy_move_and_rename_preserve_bytes(workspace):
    source = file(workspace.root / "원본 폴더" / "공백 # %.docx", bytes(range(256)))
    destination = directory(workspace.root / "복사 대상")
    moved_to = directory(workspace.root / "이동 대상")
    copied, = workspace.transfer([source], destination, copy=True)
    assert copied == destination / source.name
    assert copied.read_bytes() == source.read_bytes() == bytes(range(256))
    moved, = workspace.transfer([source], moved_to)
    assert not source.exists() and moved.read_bytes() == bytes(range(256))
    renamed = workspace.rename(moved, "새 이름 [a] # %.docx")
    assert not moved.exists() and renamed.read_bytes() == bytes(range(256))
    assert not list(workspace.root.rglob(".finderview-*"))


def test_nested_empty_folder_copy_and_parent_child_deduplication(workspace):
    source = directory(workspace.root / "폴더" / "두번째" / "세번째")
    root_folder = source.parents[1]
    child = file(root_folder / "문서.pdf")
    destination = directory(workspace.root / "대상")
    result = workspace.transfer([root_folder, child, root_folder], destination, copy=True)
    assert result == [destination / root_folder.name]
    assert (result[0] / "두번째" / "세번째").is_dir()
    assert (result[0] / "문서.pdf").read_bytes() == child.read_bytes()
    assert source.exists()
    renamed = workspace.rename(result[0], "폴더 새이름")
    assert (renamed / "두번째" / "세번째").is_dir()


def test_conflicts_default_skip_and_another_name_do_not_overwrite(workspace):
    source = file(workspace.root / "source" / "문서.txt", b"new")
    target = directory(workspace.root / "target")
    original = file(target / source.name, b"existing")
    with pytest.raises(ConflictError) as error:
        workspace.transfer([source], target)
    assert error.value.source == source and error.value.destination == original
    assert source.read_bytes() == b"new" and original.read_bytes() == b"existing"
    assert workspace.transfer([source], target, conflict="skip") == []
    renamed, = workspace.transfer([source], target, copy=True, new_name="다른 이름.txt")
    assert renamed.read_bytes() == b"new" and original.read_bytes() == b"existing"
    with pytest.raises(ConflictError):
        workspace.rename(renamed, original.name)
    assert renamed.exists()


def test_overwrite_and_explicit_delete_use_trash(workspace, trash_spy):
    source = file(workspace.root / "source" / "item.txt", b"replacement")
    target = directory(workspace.root / "target")
    existing = file(target / source.name, b"recoverable old contents")
    workspace.transfer([source], target, copy=True, conflict="overwrite")
    assert existing.read_bytes() == b"replacement"
    assert source.read_bytes() == b"replacement"
    assert trash_spy[0][0] == existing
    assert trash_spy[0][1].read_bytes() == b"recoverable old contents"
    workspace.trash([existing])
    assert not existing.exists()
    assert trash_spy[-1][1].read_bytes() == b"replacement"


def test_root_descendant_ancestor_and_other_source_are_protected(workspace, trash_spy):
    source_dir = directory(workspace.root / "parent" / "child")
    source = file(source_dir / "item.txt")
    target = directory(workspace.root / "target")
    for operation in [lambda: workspace.rename(workspace.root, "renamed"), lambda: workspace.trash([workspace.root]), lambda: workspace.transfer([workspace.root], target)]:
        with pytest.raises(BoundaryError):
            operation()
    with pytest.raises(BoundaryError):
        workspace.transfer([source_dir.parent], source_dir, copy=True)
    with pytest.raises(BoundaryError):
        workspace.transfer([source], workspace.root, conflict="overwrite", new_name="parent")
    existing_source = file(target / source.name, b"also selected")
    with pytest.raises(BoundaryError):
        workspace.transfer([source, existing_source], target, conflict="overwrite")
    assert source.read_bytes() == b"unchanged\0\xff"
    assert existing_source.read_bytes() == b"also selected"
    assert trash_spy == []


def test_external_import_copies_only_explicit_paths(workspace, tmp_path):
    outside = file(tmp_path / "외부 원본" / "파일 #.pdf", b"outside remains")
    target = directory(workspace.root / "target")
    imported, = workspace.import_external([outside], target)
    assert imported.read_bytes() == outside.read_bytes() == b"outside remains"
    with pytest.raises(BoundaryError):
        workspace.transfer([outside], target)
    with pytest.raises(BoundaryError):
        workspace.import_external([outside], tmp_path)
    with pytest.raises(BoundaryError):
        workspace.trash([outside])
    with pytest.raises(BoundaryError):
        workspace.rename(outside, "renamed")


def test_copy_and_trash_symlink_do_not_modify_its_external_target(workspace, tmp_path, trash_spy):
    outside = file(tmp_path / "secret.txt", b"outside target unchanged")
    link = workspace.root / "external link"
    link.symlink_to(outside)
    destination = directory(workspace.root / "destination")
    copied, = workspace.transfer([link], destination, copy=True)
    assert copied.is_symlink() and os.readlink(copied) == os.readlink(link)
    workspace.trash([copied, link])
    assert not os.path.lexists(link) and not os.path.lexists(copied)
    assert outside.read_bytes() == b"outside target unchanged"
    assert all(destination.is_symlink() for _, destination in trash_spy)


def test_destination_symlink_outside_root_is_rejected(workspace, tmp_path):
    outside = directory(tmp_path / "outside")
    link = workspace.root / "linked destination"
    link.symlink_to(outside, target_is_directory=True)
    source = file(workspace.root / "source.txt")
    with pytest.raises(BoundaryError):
        workspace.transfer([source], link)
    assert not list(outside.iterdir()) and source.exists()


def test_incomplete_copy_cleans_stage_and_keeps_existing_destination(workspace, monkeypatch, trash_spy):
    source = file(workspace.root / "source" / "item.txt", b"new")
    target = directory(workspace.root / "target")
    existing = file(target / source.name, b"original")

    def failed_copy(source_path, stage):
        stage.write_bytes(b"partial")
        raise OSError("simulated copy error")

    monkeypatch.setattr(workspace, "_stage_copy", failed_copy)
    with pytest.raises(OSError, match="simulated copy error"):
        workspace.transfer([source], target, copy=True, conflict="overwrite")
    assert existing.read_bytes() == b"original" and source.read_bytes() == b"new"
    assert not list(target.glob(".finderview-*")) and trash_spy == []


def test_publication_race_cannot_overwrite_new_destination(workspace, monkeypatch):
    source = file(workspace.root / "source" / "item.txt", b"new")
    target = directory(workspace.root / "target")
    destination = target / source.name
    original_rename = filesystem._rename_exclusive

    def racing_rename(staged, published):
        published.write_bytes(b"arrived concurrently")
        original_rename(staged, published)

    monkeypatch.setattr(filesystem, "_rename_exclusive", racing_rename)
    with pytest.raises(FileExistsError):
        workspace.transfer([source], target, copy=True)
    assert destination.read_bytes() == b"arrived concurrently"
    assert source.read_bytes() == b"new"
    assert not list(target.glob(".finderview-*"))


def test_invalid_names_and_same_directory_noop(workspace):
    source = file(workspace.root / "file0.txt")
    for invalid in ["", " ", ".", "..", "../escape", "nested/name", "a\0b"]:
        with pytest.raises(ValueError):
            workspace.rename(source, invalid)
    assert workspace.transfer([source], workspace.root) == []
    assert workspace.rename(source, source.name) == source
    assert source.exists()


def test_failed_trash_never_falls_back_to_permanent_deletion(workspace, monkeypatch):
    source = file(workspace.root / "source" / "item.txt", b"new")
    target = directory(workspace.root / "target")
    destination = file(target / source.name, b"must remain")

    def unavailable_trash(path):
        raise PermissionError("OS Trash unavailable")

    monkeypatch.setattr(filesystem, "send2trash", unavailable_trash)
    with pytest.raises(PermissionError, match="Trash unavailable"):
        workspace.trash([source])
    with pytest.raises(PermissionError, match="Trash unavailable"):
        workspace.transfer([source], target, copy=True, conflict="overwrite")
    assert source.read_bytes() == b"new" and destination.read_bytes() == b"must remain"
    assert not list(target.glob(".finderview-*"))


def test_case_only_rename_and_nested_external_symlink_are_preserved(workspace, tmp_path):
    original = file(workspace.root / "Case.txt", b"case data")
    changed = workspace.rename(original, "case.txt")
    assert changed.name == "case.txt" and changed.read_bytes() == b"case data"
    assert changed.name in [entry.name for entry in workspace.list_dir(workspace.root)]
    outside = directory(tmp_path / "outside")
    secret = file(outside / "secret.txt", b"do not change")
    folder = directory(workspace.root / "folder")
    (folder / "outside-link").symlink_to(outside, target_is_directory=True)
    target = directory(workspace.root / "target")
    copied, = workspace.transfer([folder], target, copy=True)
    assert (copied / "outside-link").is_symlink()
    assert os.readlink(copied / "outside-link") == str(outside)
    assert secret.read_bytes() == b"do not change"
    assert not list(workspace.root.rglob(".finderview-*"))
