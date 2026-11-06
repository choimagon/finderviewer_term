"""Headless mouse/keyboard integration tests of the real Textual application."""
from __future__ import annotations

import asyncio
from pathlib import Path
import shutil

import pytest
from textual.widgets import Input, Label

from finderview_tui.app import ConfirmDialog, ConflictDialog, ContextDialog, FileRow, FinderViewApp, InputDialog, parse_paths, safe
import finderview_tui.fs as filesystem


@pytest.fixture
def tree(tmp_path: Path) -> Path:
    root = tmp_path / "한글 테스트 #"
    (root / "alpha" / "beta" / "gamma").mkdir(parents=True)
    (root / "target").mkdir()
    (root / "alpha" / "nested.txt").write_bytes(b"nested\x00\xff")
    (root / "source.txt").write_bytes(b"original\x00\xff")
    (root / "[bold] 원고 # %.pdf").write_bytes(b"binary\xff")
    return root


def row_selector(app: FinderViewApp, column: int, name: str) -> str:
    index = next(index for index, entry in enumerate(app.columns[column].entries) if entry.name == name)
    return f"#row-{column}-{index}"


async def settle(pilot, predicate=lambda: True, timeout=3.0):
    deadline = asyncio.get_running_loop().time() + timeout
    while True:
        await pilot.pause(0.02)
        if predicate():
            return
        if asyncio.get_running_loop().time() > deadline:
            raise AssertionError("UI did not reach the expected state")


async def click_row(app, pilot, column, name, **kwargs):
    selector = row_selector(app, column, name)
    app.query_one(selector).scroll_visible(animate=False)
    await pilot.pause()
    await pilot.click(selector, offset=(2, 0), **kwargs)
    await pilot.pause()


async def fill_dialog(app, pilot, text):
    await settle(pilot, lambda: isinstance(app.screen, InputDialog))
    app.screen.query_one("#dialog-input", Input).value = text
    await pilot.press("enter")
    await pilot.pause()


async def test_click_nested_folders_toggle_and_change_ancestor(tree):
    app = FinderViewApp(tree)
    async with app.run_test(size=(150, 34)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        assert len(app.columns) == 1
        for column, name in enumerate(["alpha", "beta", "gamma"]):
            await click_row(app, pilot, column, name)
            assert len(app.columns) == column + 2
        assert app.current_directory == tree / "alpha" / "beta" / "gamma"
        assert app.columns[-1].entries == []
        await click_row(app, pilot, 1, "beta")
        assert len(app.columns) == 2
        await app.refresh_columns()
        assert len(app.columns) == 2, "Refresh must preserve a collapsed path"
        await click_row(app, pilot, 1, "beta")
        assert len(app.columns) == 3
        await click_row(app, pilot, 0, "target")
        assert [column.path for column in app.columns] == [tree, tree / "target"]


async def test_file_single_click_selects_and_double_click_opens_without_modification(tree, monkeypatch):
    app = FinderViewApp(tree)
    opened = []

    async def open_external(path):
        opened.append(path)

    monkeypatch.setattr(app, "open_external", open_external)
    file = tree / "[bold] 원고 # %.pdf"
    before = file.read_bytes()
    async with app.run_test(size=(130, 32)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await click_row(app, pilot, 0, file.name)
        assert app.selected == {file}
        assert opened == []
        label = app.query_one(row_selector(app, 0, file.name), FileRow).query_one(Label)
        assert "[bold]" in str(label.content), "Names must remain literal rather than becoming Rich markup"
        await click_row(app, pilot, 0, file.name, times=2)
        await settle(pilot, lambda: bool(opened))
        assert set(opened) == {file}
        assert file.read_bytes() == before


async def test_keyboard_right_expands_left_focuses_previous_column(tree):
    app = FinderViewApp(tree)
    async with app.run_test(size=(120, 30)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await pilot.press("right")
        await settle(pilot, lambda: len(app.columns) == 2)
        assert app.columns[1].path == tree / "alpha"
        assert app.active_column == 1
        await pilot.press("left")
        await settle(pilot)
        assert app.active_column == 0
        assert app.focused.id == "list-0"


@pytest.mark.parametrize("key,copy", [("c", True), ("x", False)])
async def test_keyboard_copy_cut_and_paste_into_clicked_folder(tree, key, copy):
    app = FinderViewApp(tree)
    source = tree / "source.txt"
    async with app.run_test(size=(140, 32)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await click_row(app, pilot, 0, source.name)
        await pilot.press(key)
        assert app.file_clipboard == ([source], copy)
        await click_row(app, pilot, 0, "target")
        await pilot.press("v")
        result = tree / "target" / source.name
        await settle(pilot, lambda: result.exists() and not app.busy)
        assert result.read_bytes() == b"original\x00\xff"
        assert source.exists() is copy
        assert (tree / "alpha" / "nested.txt").read_bytes() == b"nested\x00\xff"


async def test_f2_rename_and_q_characters_are_not_browser_shortcuts(tree):
    app = FinderViewApp(tree)
    source = tree / "source.txt"
    async with app.run_test(size=(140, 34)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await click_row(app, pilot, 0, source.name)
        await pilot.press("f2")
        await settle(pilot, lambda: isinstance(app.screen, InputDialog))
        app.screen.query_one("#dialog-input", Input).value = ""
        await pilot.press("q", "u", "i", "e", "t", ".", "t", "x", "t")
        assert app.screen.query_one("#dialog-input", Input).value == "quiet.txt"
        await pilot.press("enter")
        result = tree / "quiet.txt"
        await settle(pilot, lambda: result.exists() and not app.busy)
        assert result.read_bytes() == b"original\x00\xff"
        assert not source.exists()


async def test_right_click_context_trash_cancel_then_confirm(tree, tmp_path, monkeypatch):
    app = FinderViewApp(tree)
    source = tree / "source.txt"
    trashed = []

    def fake_trash(path):
        original = Path(path)
        destination = tmp_path / "trash-copy.txt"
        shutil.move(str(original), destination)
        trashed.append(destination)

    monkeypatch.setattr(filesystem, "send2trash", fake_trash)
    async with app.run_test(size=(140, 40)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await click_row(app, pilot, 0, source.name, button=3)
        await settle(pilot, lambda: isinstance(app.screen, ContextDialog))
        assert await pilot.click("#ctx-trash")
        await settle(pilot, lambda: isinstance(app.screen, ConfirmDialog))
        assert await pilot.click("#confirm-no")
        await settle(pilot, lambda: len(app.screen_stack) == 1)
        assert source.exists() and not trashed
        await pilot.press("delete")
        await settle(pilot, lambda: isinstance(app.screen, ConfirmDialog))
        assert await pilot.click("#confirm-yes")
        await settle(pilot, lambda: not source.exists() and not app.busy)
        assert len(trashed) == 1 and trashed[0].read_bytes() == b"original\x00\xff"


@pytest.mark.parametrize("decision", ["rename", "skip", "overwrite", "cancel"])
async def test_paste_name_conflict_choices(tree, tmp_path, monkeypatch, decision):
    app = FinderViewApp(tree)
    source = tree / "source.txt"
    destination = tree / "target" / source.name
    destination.write_bytes(b"existing target")
    trash_contents = []

    def fake_trash(path):
        target = Path(path)
        trash_contents.append(target.read_bytes())
        shutil.move(str(target), tmp_path / "replaced-target.txt")

    monkeypatch.setattr(filesystem, "send2trash", fake_trash)
    async with app.run_test(size=(150, 38)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await click_row(app, pilot, 0, source.name)
        await pilot.press("c")
        await click_row(app, pilot, 0, "target")
        await pilot.press("v")
        await settle(pilot, lambda: isinstance(app.screen, ConflictDialog))
        assert await pilot.click(f"#{decision}")
        if decision == "rename":
            await fill_dialog(app, pilot, "renamed #.txt")
            await settle(pilot, lambda: (tree / "target" / "renamed #.txt").exists() and not app.busy)
            assert (tree / "target" / "renamed #.txt").read_bytes() == source.read_bytes()
            assert destination.read_bytes() == b"existing target"
        elif decision == "overwrite":
            await settle(pilot, lambda: not app.busy and len(app.screen_stack) == 1)
            assert destination.read_bytes() == source.read_bytes()
            assert trash_contents == [b"existing target"]
        else:
            await settle(pilot, lambda: not app.busy and len(app.screen_stack) == 1)
            assert destination.read_bytes() == b"existing target" and not trash_contents
        assert source.read_bytes() == b"original\x00\xff"


def test_external_path_parser_and_display_never_evaluate_shell(tmp_path):
    filename = tmp_path / "공백 # $(touch HACKED).pdf"
    filename.write_bytes(b"safe")
    assert parse_paths(str(filename), tmp_path) == [filename]
    assert parse_paths("'" + str(filename) + "'", tmp_path) == [filename]
    assert parse_paths(filename.as_uri(), tmp_path) == [filename]
    assert not (tmp_path / "HACKED").exists()
    assert safe("x\x1b[31m\ny") == "x\\x1b[31m\\x0ay"


@pytest.mark.parametrize("copy", [False, True])
async def test_mouse_drag_moves_or_modifier_copies_to_folder(tree, copy):
    app = FinderViewApp(tree)
    source = tree / "source.txt"
    target = tree / "target" / source.name
    async with app.run_test(size=(140, 34)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        source_row = row_selector(app, 0, source.name)
        target_row = row_selector(app, 0, "target")
        await pilot.mouse_down(source_row, offset=(3, 0), control=copy)
        await pilot.pause(0.05)
        await pilot.hover(target_row, offset=(3, 0))
        await pilot.pause(0.05)
        assert app.query_one(source_row, FileRow).dragging
        await pilot.mouse_up(target_row, offset=(3, 0), control=copy)
        await settle(pilot, lambda: target.exists() and not app.busy)
        assert target.read_bytes() == b"original\x00\xff"
        assert source.exists() is copy


async def test_external_import_dialog_confirms_copy_and_preserves_original(tree, tmp_path):
    external = tmp_path / "외부 문서 #.pdf"
    external.write_bytes(b"external\0\xff")
    app = FinderViewApp(tree)
    async with app.run_test(size=(150, 35)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await click_row(app, pilot, 0, "target")
        await pilot.press("i")
        await fill_dialog(app, pilot, str(external))
        await settle(pilot, lambda: isinstance(app.screen, ConfirmDialog))
        assert await pilot.click("#confirm-yes")
        destination = tree / "target" / external.name
        await settle(pilot, lambda: destination.exists() and not app.busy)
        assert external.read_bytes() == destination.read_bytes() == b"external\0\xff"


async def test_parent_and_child_selection_moves_once_without_partial_failure(tree):
    app = FinderViewApp(tree)
    async with app.run_test(size=(150, 36)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await click_row(app, pilot, 0, "alpha")
        await click_row(app, pilot, 1, "nested.txt", control=True)
        assert app.selected == {tree / "alpha", tree / "alpha" / "nested.txt"}
        await pilot.press("x")
        await click_row(app, pilot, 0, "target")
        await pilot.press("v")
        moved = tree / "target" / "alpha"
        await settle(pilot, lambda: moved.exists() and not app.busy)
        assert (moved / "nested.txt").read_bytes() == b"nested\x00\xff"
        assert (moved / "beta" / "gamma").is_dir()
        assert not (tree / "alpha").exists()
        assert app.file_clipboard is None, "Successful deduplicated move clears the cut clipboard"


async def test_overwrite_cannot_replace_another_selected_original(tree, tmp_path, monkeypatch):
    source = tree / "source.txt"
    other_source = tree / "target" / source.name
    other_source.write_bytes(b"selected original must remain")
    trashed = []

    def fake_trash(value):
        target = Path(value)
        trashed.append(target)
        shutil.move(str(target), tmp_path / "unexpected-trash.txt")

    monkeypatch.setattr(filesystem, "send2trash", fake_trash)
    app = FinderViewApp(tree)
    async with app.run_test(size=(150, 40)) as pilot:
        await settle(pilot, lambda: app.browser_ready)
        await click_row(app, pilot, 0, "target")
        await click_row(app, pilot, 1, source.name)
        await click_row(app, pilot, 0, source.name, control=True)
        assert app.selected == {source, other_source}
        await pilot.press("c")
        await click_row(app, pilot, 1, source.name, button=3)
        await settle(pilot, lambda: isinstance(app.screen, ContextDialog))
        assert await pilot.click("#ctx-paste")
        await settle(pilot, lambda: isinstance(app.screen, ConflictDialog))
        assert await pilot.click("#overwrite")
        await settle(pilot, lambda: not app.busy and len(app.screen_stack) == 1)
        assert source.read_bytes() == b"original\x00\xff"
        assert other_source.read_bytes() == b"selected original must remain"
        assert trashed == [], "All selected-source protections must run before any replacement"


async def test_long_filename_is_visible_in_one_line(tmp_path):
    from textual.geometry import Region
    from textual.widgets import Label
    from finderview_tui.app import FinderViewApp, FileRow
    (tmp_path / 'column-explorer-very-long-0.1.5.vsix').write_text('fixture')
    app = FinderViewApp(tmp_path)
    async with app.run_test(size=(120, 28)) as pilot:
        await pilot.pause()
        label = app.query_one(FileRow).query_one(Label)
        rendered = label.render_lines(Region(0, 0, label.size.width, 1))[0].text
        assert 'column-explorer' in rendered
        assert rendered.endswith('…')
