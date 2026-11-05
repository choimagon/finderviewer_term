"""Filesystem operations for FinderView's terminal UI.

Every mutation is explicit. Existing destinations are never silently replaced;
requested replacements go to the operating system Trash, without a permanent
remove fallback. Navigation and mutation destinations stay inside ``root``.
"""
from __future__ import annotations

import ctypes
import errno
import os
from pathlib import Path
import shutil
import stat
import sys
from dataclasses import dataclass
from typing import Literal
from uuid import uuid4

from send2trash import send2trash

SortOrder = Literal["folders", "name", "recent"]
ConflictPolicy = Literal["error", "overwrite", "skip"]


@dataclass(frozen=True, slots=True)
class Entry:
    path: Path
    name: str
    is_dir: bool
    size: int
    mtime: float
    is_symlink: bool = False


class BoundaryError(PermissionError):
    """An operation would leave the selected workspace or modify its root."""


class ConflictError(FileExistsError):
    def __init__(self, source: Path, destination: Path):
        self.source = source
        self.destination = destination
        super().__init__(f"같은 이름이 이미 있습니다: {destination.name}")


class PartialOperationError(OSError):
    def __init__(self, completed: list[Path], cause: Exception):
        self.completed = completed
        self.cause = cause
        super().__init__(f"{len(completed)}개 완료 후 중단했습니다: {cause}")


@dataclass(slots=True)
class _Transfer:
    source: Path
    destination: Path
    expected: tuple[int, int, int, int, int] | None


def _exists(path: Path) -> bool:
    return os.path.lexists(path)


def _signature(path: Path) -> tuple[int, int, int, int, int]:
    info = path.lstat()
    return info.st_dev, info.st_ino, info.st_mode, info.st_size, info.st_mtime_ns


def _rename_exclusive(source: Path, destination: Path) -> None:
    """Native no-clobber rename, including the final publication of copies."""
    if os.name == "nt":
        os.rename(source, destination)  # Windows raises if destination exists.
        return
    library = ctypes.CDLL(None, use_errno=True)
    if sys.platform == "darwin" and hasattr(library, "renamex_np"):
        rename = library.renamex_np
        rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
        rename.restype = ctypes.c_int
        result = rename(os.fsencode(source), os.fsencode(destination), 0x00000004)
    elif sys.platform.startswith("linux") and hasattr(library, "renameat2"):
        rename = library.renameat2
        rename.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        rename.restype = ctypes.c_int
        result = rename(-100, os.fsencode(source), -100, os.fsencode(destination), 1)
    else:
        raise OSError("이 환경은 안전한 덮어쓰기 방지 이동을 지원하지 않습니다.")
    if result:
        code = ctypes.get_errno()
        if code == errno.EXDEV:
            raise OSError(code, "다른 디스크로의 이동은 지원하지 않습니다. 복사 후 원본을 휴지통으로 이동하세요.", str(source))
        raise OSError(code, os.strerror(code), str(destination))


def _remove_stage(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink(missing_ok=True)
    elif path.exists():
        shutil.rmtree(path)


class FileManager:
    def __init__(self, root: Path | str):
        self.root = Path(root).expanduser().resolve(strict=True)
        if not self.root.is_dir():
            raise NotADirectoryError(str(self.root))

    def _absolute(self, path: Path | str) -> Path:
        candidate = Path(path).expanduser()
        return Path(os.path.abspath(candidate if candidate.is_absolute() else self.root / candidate))

    def validate(self, path: Path | str, directory: bool = False) -> Path:
        """Validate navigation, following symlinks only inside the root."""
        candidate = self._absolute(path)
        resolved = candidate.resolve(strict=True)
        if not resolved.is_relative_to(self.root):
            raise BoundaryError("선택한 시작 폴더 밖으로는 이동할 수 없습니다.")
        if directory and not resolved.is_dir():
            raise NotADirectoryError(str(candidate))
        return candidate

    def _mutable(self, path: Path | str, *, external: bool = False) -> Path:
        """Validate parent rather than dereferencing the final symlink itself."""
        candidate = self._absolute(path)
        candidate = candidate.parent.resolve(strict=True) / candidate.name
        if not external and not candidate.is_relative_to(self.root):
            raise BoundaryError("선택한 시작 폴더 밖의 항목은 변경할 수 없습니다.")
        if candidate == self.root:
            raise BoundaryError("시작 폴더 자체는 이동·삭제·이름 변경할 수 없습니다.")
        candidate.lstat()
        return candidate

    def list_dir(self, path: Path | str, show_hidden: bool = True, sort: SortOrder = "folders") -> list[Entry]:
        directory = self.validate(path, directory=True)
        if sort not in {"folders", "name", "recent"}:
            raise ValueError("정렬은 folders, name, recent 중 하나여야 합니다.")
        entries: list[Entry] = []
        for child in directory.iterdir():
            if not show_hidden and child.name.startswith("."):
                continue
            try:
                info = child.lstat()
                is_link = stat.S_ISLNK(info.st_mode)
                entries.append(Entry(child, child.name, child.is_dir(), info.st_size, info.st_mtime, is_link))
            except FileNotFoundError:
                continue  # An external program removed an entry mid-refresh.
        name_key = lambda entry: (entry.name.casefold(), entry.name)
        if sort == "folders":
            return sorted(entries, key=lambda entry: (not entry.is_dir, *name_key(entry)))
        if sort == "recent":
            return sorted(entries, key=lambda entry: (-entry.mtime, *name_key(entry)))
        return sorted(entries, key=name_key)

    @staticmethod
    def _name(name: str) -> str:
        if not isinstance(name, str) or not name.strip() or name in {".", ".."} or "\0" in name:
            raise ValueError("파일 또는 폴더 이름을 입력하세요.")
        if Path(name).name != name or (os.name == "nt" and any(char in name for char in '<>:"/\\|?*')):
            raise ValueError("경로 대신 이름만 입력하세요.")
        return name

    def _selection(self, sources: list[Path], external: bool = False) -> list[Path]:
        if not sources:
            raise ValueError("항목을 선택하세요.")
        selected = list(dict.fromkeys(self._mutable(source, external=external) for source in sources))
        return [source for source in selected if not any(
            parent != source and not parent.is_symlink() and parent.is_dir() and source.is_relative_to(parent)
            for parent in selected
        )]

    @staticmethod
    def _protect_source(source: Path, destination: Path) -> None:
        if source == destination or source.is_relative_to(destination):
            raise BoundaryError("원본 또는 원본이 들어 있는 상위 폴더를 덮어쓸 수 없습니다.")
        if _exists(destination):
            try:
                if os.path.samefile(source, destination):
                    raise BoundaryError("원본 파일 자체를 덮어쓸 수 없습니다.")
            except FileNotFoundError:
                pass

    def transfer(self, sources: list[Path], destination: Path, copy: bool = False,
                 conflict: ConflictPolicy = "error", new_name: str | None = None) -> list[Path]:
        """Copy or move selected workspace entries into a workspace directory."""
        return self._transfer(sources, destination, copy, conflict, new_name, external=False)

    def import_external(self, sources: list[Path], destination: Path,
                        conflict: ConflictPolicy = "error", new_name: str | None = None) -> list[Path]:
        """Copy explicit user-provided paths without modifying their originals."""
        return self._transfer(sources, destination, True, conflict, new_name, external=True)

    def _transfer(self, sources: list[Path], destination: Path, copy: bool,
                  conflict: ConflictPolicy, new_name: str | None, external: bool) -> list[Path]:
        if conflict not in {"error", "overwrite", "skip"}:
            raise ValueError("알 수 없는 이름 충돌 처리 방식입니다.")
        selected = self._selection(sources, external)
        directory = self.validate(destination, directory=True).resolve(strict=True)
        if new_name is not None and len(selected) != 1:
            raise ValueError("다른 이름 저장은 한 항목씩 선택하세요.")
        plan: list[_Transfer] = []
        names: set[str] = set()
        for source in selected:
            target = directory / (self._name(new_name) if new_name is not None else source.name)
            if source == target:
                continue
            if source.is_dir() and not source.is_symlink() and directory.is_relative_to(source):
                raise BoundaryError("폴더를 자기 자신이나 하위 폴더로 옮기거나 복사할 수 없습니다.")
            if target == self.root:
                raise BoundaryError("시작 폴더를 교체할 수 없습니다.")
            key = target.name if sys.platform.startswith("linux") else target.name.casefold()
            if key in names:
                raise ValueError(f"선택한 항목들의 대상 이름이 중복됩니다: {target.name}")
            names.add(key)
            for original in selected:
                self._protect_source(original, target)
            if not copy and source.lstat().st_dev != directory.stat().st_dev:
                raise OSError(errno.EXDEV, "다른 디스크로 이동할 때는 복사 후 원본을 휴지통으로 이동하세요.", str(source))
            expected = _signature(target) if _exists(target) else None
            if expected is not None:
                if conflict == "skip":
                    continue
                if conflict == "error":
                    raise ConflictError(source, target)
            plan.append(_Transfer(source, target, expected))
        completed: list[Path] = []
        for item in plan:
            try:
                self.validate(directory, directory=True)
                self._mutable(item.source, external=external)
                if copy:
                    stage = directory / f".finderview-copy-{uuid4().hex}"
                    try:
                        self._stage_copy(item.source, stage)
                        self._replace(item.destination, item.expected)
                        _rename_exclusive(stage, item.destination)
                    finally:
                        _remove_stage(stage)
                else:
                    self._replace(item.destination, item.expected)
                    _rename_exclusive(item.source, item.destination)
                completed.append(item.destination)
            except Exception as error:
                if completed:
                    raise PartialOperationError(completed, error) from error
                raise
        return completed

    @staticmethod
    def _stage_copy(source: Path, stage: Path) -> None:
        if source.is_symlink():
            os.symlink(os.readlink(source), stage, target_is_directory=source.is_dir())
        elif source.is_dir():
            shutil.copytree(source, stage, symlinks=True)
        elif source.is_file():
            with source.open("rb") as original, stage.open("xb") as target:
                shutil.copyfileobj(original, target, length=1024 * 1024)
            shutil.copystat(source, stage, follow_symlinks=False)
        else:
            raise ValueError("일반 파일·폴더·심볼릭 링크만 복사할 수 있습니다.")

    @staticmethod
    def _replace(destination: Path, expected: tuple[int, int, int, int, int] | None) -> None:
        if expected is None:
            if _exists(destination):
                raise FileExistsError(f"대상이 변경되었습니다. 다시 시도하세요: {destination.name}")
            return
        if not _exists(destination) or _signature(destination) != expected:
            raise FileExistsError("덮어쓰기 확인 후 대상이 변경되었습니다. 다시 시도하세요.")
        send2trash(str(destination))

    def rename(self, source: Path, new_name: str, overwrite: bool = False) -> Path:
        original = self._mutable(source)
        target = original.parent / self._name(new_name)
        if original == target:
            return original
        if target == self.root:
            raise BoundaryError("시작 폴더를 교체할 수 없습니다.")
        # A case-only rename on a case-insensitive filesystem needs two steps.
        if _exists(target) and os.path.samefile(original, target):
            stage = original.parent / f".finderview-rename-{uuid4().hex}"
            _rename_exclusive(original, stage)
            try:
                _rename_exclusive(stage, target)
            except Exception:
                _rename_exclusive(stage, original)
                raise
            return target
        self._protect_source(original, target)
        expected = _signature(target) if _exists(target) else None
        if expected is not None and not overwrite:
            raise ConflictError(original, target)
        if original.lstat().st_dev != original.parent.stat().st_dev:
            raise OSError(errno.EXDEV, "마운트된 폴더 자체는 이름을 바꿀 수 없습니다.", str(original))
        self._replace(target, expected)
        _rename_exclusive(original, target)
        return target

    def trash(self, paths: list[Path]) -> None:
        selected = self._selection(paths)
        completed: list[Path] = []
        for source in selected:
            try:
                self._mutable(source)
                send2trash(str(source))
                completed.append(source)
            except Exception as error:
                if completed:
                    raise PartialOperationError(completed, error) from error
                raise
