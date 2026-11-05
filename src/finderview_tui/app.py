"""Interactive Miller-column file explorer; no shell commands are evaluated."""
from __future__ import annotations

import asyncio
from dataclasses import dataclass, field
from datetime import datetime
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
from urllib.parse import unquote, urlparse

from rich.text import Text
from textual import events
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, HorizontalScroll, Vertical, VerticalScroll
from textual.message import Message
from textual.screen import ModalScreen
from textual.widgets import Button, Footer, Input, Label, ListItem, ListView, Select, Static

from .fs import ConflictError, Entry, FileManager


def safe(value: object) -> str:
    return "".join(character if ord(character) >= 32 and ord(character) != 127 else f"\\x{ord(character):02x}" for character in str(value))


def parse_paths(text: str, base: Path) -> list[Path]:
    text = text.strip()
    if not text or len(text) > 65536:
        raise ValueError("파일 경로를 입력하세요.")
    direct = Path(text).expanduser()
    if not direct.is_absolute():
        direct = base / direct
    if direct.exists():
        return [direct.absolute()]
    tokens = shlex.split(text, posix=os.name != "nt")
    result: list[Path] = []
    for token in tokens:
        if token.startswith("file:"):
            parsed = urlparse(token)
            if parsed.netloc not in ("", "localhost"):
                raise ValueError("로컬 파일 경로만 가져올 수 있습니다.")
            token = unquote(parsed.path)
        token = token.strip('"') if os.name == "nt" else token
        path = Path(token).expanduser()
        if not path.is_absolute():
            path = base / path
        if not path.exists() and not path.is_symlink():
            raise FileNotFoundError(f"경로를 찾을 수 없습니다: {safe(path)}")
        result.append(path.absolute())
    if not result:
        raise ValueError("파일 경로를 입력하세요.")
    return list(dict.fromkeys(result))


class InputDialog(ModalScreen[str | None]):
    BINDINGS = [Binding("escape", "cancel", show=False)]

    def __init__(self, title: str, value: str = "", hint: str = ""):
        super().__init__()
        self.prompt, self.value, self.hint = title, value, hint

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog"):
            yield Static(Text(self.prompt, style="bold"), classes="dialog-title")
            if self.hint:
                yield Static(self.hint, classes="dialog-hint", markup=False)
            yield Input(self.value, id="dialog-input", select_on_focus=True)
            with Horizontal(classes="dialog-buttons"):
                yield Button("확인", id="dialog-ok", variant="primary")
                yield Button("취소", id="dialog-cancel")

    def on_mount(self) -> None:
        self.query_one(Input).focus()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        event.stop()
        if event.value.strip():
            self.dismiss(event.value)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        event.stop()
        if event.button.id == "dialog-ok":
            value = self.query_one(Input).value
            if value.strip():
                self.dismiss(value)
        else:
            self.dismiss(None)

    def action_cancel(self) -> None:
        self.dismiss(None)


class ConfirmDialog(ModalScreen[bool]):
    BINDINGS = [Binding("escape", "cancel", show=False)]

    def __init__(self, prompt: str):
        super().__init__()
        self.prompt = prompt

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog"):
            yield Static(Text(self.prompt), classes="dialog-title")
            with Horizontal(classes="dialog-buttons"):
                yield Button("확인", id="confirm-yes", variant="warning")
                yield Button("취소", id="confirm-no", variant="primary")

    def on_mount(self) -> None:
        self.query_one("#confirm-no", Button).focus()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        event.stop()
        self.dismiss(event.button.id == "confirm-yes")

    def action_cancel(self) -> None:
        self.dismiss(False)


class ConflictDialog(ModalScreen[str]):
    BINDINGS = [Binding("escape", "cancel", show=False)]

    def __init__(self, destination: Path):
        super().__init__()
        self.destination = destination

    def compose(self) -> ComposeResult:
        with Vertical(classes="dialog"):
            yield Static(Text(f"같은 이름이 있습니다\n{safe(self.destination)}", style="bold"), classes="dialog-title")
            yield Static("덮어쓰면 기존 항목을 휴지통에 보관한 뒤 교체합니다.", markup=False)
            with Horizontal(classes="dialog-buttons"):
                yield Button("덮어쓰기", id="overwrite", variant="warning")
                yield Button("다른 이름", id="rename", variant="primary")
                yield Button("건너뛰기", id="skip")
                yield Button("취소", id="cancel")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        event.stop()
        self.dismiss(event.button.id or "cancel")

    def action_cancel(self) -> None:
        self.dismiss("cancel")


class ContextDialog(ModalScreen[str | None]):
    BINDINGS = [Binding("escape", "cancel", show=False)]

    def __init__(self, title: str):
        super().__init__()
        self.prompt = title

    def compose(self) -> ComposeResult:
        with VerticalScroll(classes="dialog context-dialog"):
            yield Static(Text(safe(self.prompt), style="bold"), classes="dialog-title")
            for action, title in [("open", "열기"), ("cut", "잘라내기  x"), ("copy", "복사  c"), ("paste", "여기에 붙여넣기  v"), ("move", "다른 폴더로 이동  m"), ("rename", "이름 변경  F2"), ("trash", "삭제 (휴지통)  Delete"), ("import", "외부 파일 가져오기  i")]:
                yield Button(title, id=f"ctx-{action}", variant="error" if action == "trash" else "default")
            yield Button("닫기", id="ctx-cancel")

    def on_mount(self) -> None:
        self.query_one("#ctx-open",Button).focus()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        event.stop()
        action = (event.button.id or "ctx-cancel").removeprefix("ctx-")
        self.dismiss(None if action == "cancel" else action)

    def action_cancel(self) -> None:
        self.dismiss(None)


@dataclass
class Column:
    path: Path
    entries: list[Entry] = field(default_factory=list)
    selected: Path | None = None
    collapsed: bool = False
    error: str = ""


class FileRow(ListItem):
    class Picked(Message):
        def __init__(self, row: FileRow, *, context: bool = False, double: bool = False, multi: bool = False, shift: bool = False):
            super().__init__()
            self.row, self.context, self.double, self.multi, self.shift = row, context, double, multi, shift

    def __init__(self, entry: Entry, column: int, index: int, expanded: bool = False):
        super().__init__(id=f"row-{column}-{index}")
        self.entry, self.column, self.index = entry, column, index
        self.expanded = expanded
        self.drag_origin: tuple[int, int] | None = None
        self.dragging = False

    def compose(self) -> ComposeResult:
        icon = "⌄ " if self.expanded else "› " if self.entry.is_dir else "· "
        text = Text(icon, style="bold #77b7ed" if self.entry.is_dir else "#9caec3", no_wrap=True, overflow="ellipsis")
        text.append(safe(self.entry.name))
        if self.entry.is_symlink:
            text.append(" ↗", style="dim")
        yield Label(text)

    def on_click(self, event: events.Click) -> None:
        event.stop()
        event.prevent_default()
        if time.monotonic() < self.app.suppress_click_until:
            return
        self.parent.focus()
        self.parent.index = self.index
        self.post_message(self.Picked(self, context=event.button == 3, double=event.chain >= 2, multi=event.ctrl or event.meta, shift=event.shift))

    def on_mouse_down(self, event: events.MouseDown) -> None:
        if event.button == 1:
            self.drag_origin = (event.screen_x, event.screen_y)
            self.dragging = False
            self.capture_mouse()

    def on_mouse_move(self, event: events.MouseMove) -> None:
        if self.drag_origin and (abs(event.screen_x-self.drag_origin[0]) + abs(event.screen_y-self.drag_origin[1]) > 1):
            self.dragging = True
            self.app.query_one("#path", Static).update(Text(f"이동할 폴더에 놓으세요: {safe(self.entry.name)}", style="bold #7ac7ff"))

    def on_mouse_up(self, event: events.MouseUp) -> None:
        self.release_mouse()
        if self.drag_origin and self.dragging:
            self.app.suppress_click_until = time.monotonic() + 0.25
            target, _ = self.screen.get_widget_at(event.screen_x, event.screen_y)
            while target is not None and not isinstance(target, (FileRow, FileColumn)):
                target = target.parent
            destination = target.entry.path if isinstance(target, FileRow) and target.entry.is_dir else target.parent.parent.path if isinstance(target, FileRow) else target.path if isinstance(target, FileColumn) else None
            if destination:
                sources = list(self.app.selected) if self.entry.path in self.app.selected else [self.entry.path]
                self.app.run_worker(self.app.transfer_to(sources, destination, copy=event.ctrl or event.meta))
        self.drag_origin = None
        self.dragging = False


class EntryList(ListView):
    BINDINGS = [Binding("up", "previous", show=False), Binding("down", "next", show=False), Binding("enter", "activate", show=False), Binding("right", "expand", show=False), Binding("left", "left", show=False), Binding("space", "mark", show=False)]

    def __init__(self, *rows: FileRow, column: int):
        super().__init__(*rows, id=f"list-{column}")
        self.column = column

    def current(self) -> FileRow | None:
        return self.highlighted_child if isinstance(self.highlighted_child, FileRow) else None

    async def action_previous(self) -> None:
        self.index = max(0, (self.index or 0)-1)
        await self.moved()

    async def action_next(self) -> None:
        self.index = min(len(self.children)-1, (self.index or 0)+1) if self.children else None
        await self.moved()

    async def moved(self) -> None:
        if row := self.current():
            self.app.active_column = self.column
            self.app.selected = {row.entry.path}
            self.app.anchor = (self.column, row.index)
            self.app.update_details(row.entry)
            self.app.sync_marks()

    async def action_activate(self) -> None:
        if row := self.current():
            await self.app.activate(row.entry, self.column, toggle=True, open_file=True)

    async def action_expand(self) -> None:
        if row := self.current():
            if row.entry.is_dir:
                await self.app.activate(row.entry, self.column, toggle=False)
            else:
                self.app.update_details(row.entry)

    def action_left(self) -> None:
        self.app.focus_column(max(0, self.column-1))

    def action_mark(self) -> None:
        if row := self.current():
            self.app.toggle_mark(row.entry.path)


class FileColumn(Vertical):
    def __init__(self, model: Column, index: int, expanded: Path | None):
        super().__init__(id=f"column-{index}", classes="file-column")
        self.path, self.model, self.index, self.expanded = model.path, model, index, expanded

    def compose(self) -> ComposeResult:
        yield Static(Text(f"{safe(self.path.name or self.path)}  ·  {len(self.model.entries)}", style="bold"), classes="column-title")
        if not self.model.entries:
            yield Static(Text(self.model.error or "빈 폴더입니다", style="dim"), classes="empty")
        yield EntryList(*(FileRow(entry, self.index, index, self.expanded == entry.path) for index, entry in enumerate(self.model.entries)), column=self.index)


class FinderViewApp(App):
    TITLE = "FinderView Terminal"
    CSS = """
    Screen { background: #141b25; color: #d4ddea; }
    #toolbar { height: 3; background: #1d2938; padding: 0 1; align-vertical: middle; }
    #brand { width: 23; text-style: bold; color: #a7d7ff; padding: 1 0; }
    #toolbar Button { min-width: 9; margin: 0 1 0 0; }
    #sort { width: 20; }
    #path { height: 1; padding: 0 1; color: #9cbbd7; background: #182332; }
    #columns { height: 1fr; width: 100%; }
    .file-column { width: 32; height: 100%; border-right: solid #354356; }
    .column-title { height: 1; background: #1d2938; color: #c7dbef; padding: 0 1; }
    EntryList { height: 1fr; background: #141b25; padding: 0; }
    FileRow { height: 1; padding: 0 1; }
    FileRow Label { width: 100%; height: 1; text-wrap: nowrap; text-overflow: ellipsis; }
    FileRow.-highlight { background: #26384e; }
    FileRow.marked { background: #26547f; color: white; text-style: bold; }
    .empty { height: 2; padding: 0 1; }
    #details { height: 3; padding: 0 1; background: #1d2938; color: #bcd1e6; }
    Footer { background: #1a2737; }
    ModalScreen { align: center middle; background: #000000 55%; }
    .dialog { width: 76; max-width: 95%; height: auto; max-height: 95%; border: round #6ca9e4; padding: 1 2; background: #1b2a3b; }
    .dialog-title { height: auto; margin-bottom: 1; }
    .dialog-hint { height: auto; color: #a8bfd6; margin-bottom: 1; }
    .dialog-buttons { height: auto; align-horizontal: right; margin-top: 1; }
    .dialog-buttons Button { min-width: 10; margin-left: 1; }
    .context-dialog { width: 42; }
    .context-dialog Button { width: 100%; margin-bottom: 0; height: 3; }
    """
    BINDINGS = [
        Binding("ctrl+q", "quit", "종료", priority=True), Binding("q", "close_browser", show=False),
        Binding("f5", "refresh", "새로고침"), Binding("ctrl+l", "location", "경로"),
        Binding("c", "copy", "복사"), Binding("x", "cut", "잘라내기"), Binding("v", "paste", "붙여넣기"),
        Binding("m", "move", "이동"), Binding("f2", "rename", "이름 변경"), Binding("delete,ctrl+d", "trash", "삭제"),
        Binding("i", "import", "가져오기"), Binding("f10", "context", "메뉴"), Binding("h", "hidden", show=False),
        Binding("backspace", "parent", show=False), Binding("ctrl+a", "select_all", show=False),
    ]

    def __init__(self, root: Path, *, event_log: Path | None = None):
        super().__init__()
        self.manager = FileManager(root)
        self.columns: list[Column] = []
        self.active_column = 0
        self.selected: set[Path] = set()
        self.anchor: tuple[int, int] | None = None
        self.file_clipboard: tuple[list[Path], bool] | None = None
        self.show_hidden = True
        self.sort_order = "folders"
        self.busy = False
        self.suppress_click_until = 0.0
        self.browser_ready = False
        self.browser_rendering = False
        self._operation_lock = asyncio.Lock()
        self._navigation_lock = asyncio.Lock()
        self.event_log = event_log

    def compose(self) -> ComposeResult:
        with Horizontal(id="toolbar"):
            yield Static("▱ FinderView", id="brand")
            yield Button("상위", id="parent")
            yield Button("새로고침", id="refresh")
            yield Button("점 파일 ON", id="hidden")
            yield Select([("폴더 우선", "folders"), ("이름순", "name"), ("최근 수정순", "recent")], value="folders", allow_blank=False, id="sort")
        yield Static("불러오는 중…", id="path", markup=False)
        yield HorizontalScroll(id="columns")
        yield Static("클릭: 폴더 펼치기/접기 · 우클릭: 메뉴 · 외부 파일: i 후 경로 드래그", id="details", markup=False)
        yield Footer()

    def record(self, action: str, **values: object) -> None:
        if self.event_log:
            import json
            with self.event_log.open("a", encoding="utf8") as stream:
                stream.write(json.dumps({"action":action, **values}, ensure_ascii=False) + "\n")

    async def on_mount(self) -> None:
        await self.refresh_columns()
        self.browser_ready = True
        self.set_interval(2.0, self.auto_refresh)

    async def load_column(self, path: Path) -> Column:
        try:
            entries = await asyncio.to_thread(self.manager.list_dir, path, self.show_hidden, self.sort_order)
            return Column(path, entries)
        except OSError as error:
            return Column(path, error=safe(error))

    async def render_columns(self, focus: int | None = None) -> None:
        self.browser_rendering = True
        container = self.query_one("#columns", HorizontalScroll)
        await container.remove_children()
        for index, model in enumerate(self.columns):
            expanded = self.columns[index+1].path if index+1 < len(self.columns) else None
            widget = FileColumn(model, index, expanded)
            await container.mount(widget)
            view = widget.query_one(EntryList)
            match = next((position for position, entry in enumerate(model.entries) if entry.path == model.selected), None)
            view.index = match if match is not None else 0 if model.entries else None
        self.selected.intersection_update({entry.path for model in self.columns for entry in model.entries})
        self.sync_marks()
        self.browser_rendering = False
        if focus is not None:
            self.focus_column(min(focus,len(self.columns)-1))
        self.query_one("#path", Static).update(Text(safe(self.current_directory)))

    def focus_column(self, index: int) -> None:
        self.active_column = max(0,min(index,len(self.columns)-1))
        view = self.query_one(f"#list-{self.active_column}", EntryList)
        view.focus()
        view.scroll_visible(animate=False)

    @property
    def current_directory(self) -> Path:
        return self.columns[min(self.active_column,len(self.columns)-1)].path if self.columns else self.manager.root

    async def refresh_columns(self) -> None:
        async with self._navigation_lock:
            old = self.columns
            models: list[Column] = []
            path = self.manager.root
            for index in range(max(1,len(old))):
                model = await self.load_column(path)
                models.append(model)
                previous = old[index] if index < len(old) else None
                entry = next((entry for entry in model.entries if previous and entry.path == previous.selected), None)
                if not entry:
                    break
                model.selected, model.collapsed = entry.path, previous.collapsed
                if not entry.is_dir or model.collapsed or index+1 >= len(old):
                    break
                path = entry.path
            self.columns = models
            await self.render_columns(self.active_column)
            selected_entry=next((entry for model in self.columns for entry in model.entries if entry.path in self.selected),None)
            if selected_entry is not None:
                self.update_details(selected_entry)
            elif not self.selected:
                self.query_one("#details",Static).update(Text(f"{safe(self.current_directory)}\n클릭: 폴더 펼치기/접기 · 우클릭: 메뉴\n외부 파일: i 후 경로 드래그 · Ctrl+D: 휴지통"))

    async def auto_refresh(self) -> None:
        if self.busy or len(self.screen_stack) > 1 or not self.browser_ready:
            return
        # Only rebuild when entries or their metadata actually changed.
        for model in self.columns:
            changed = await self.load_column(model.path)
            if changed.entries != model.entries or changed.error != model.error:
                await self.refresh_columns()
                break

    def sync_marks(self) -> None:
        for row in self.query(FileRow):
            row.set_class(row.entry.path in self.selected,"marked")

    def update_details(self, entry: Entry) -> None:
        modified = datetime.fromtimestamp(entry.mtime).strftime("%Y-%m-%d %H:%M")
        size = "폴더" if entry.is_dir else f"{entry.size:,} bytes"
        self.query_one("#details",Static).update(Text(f"{safe(entry.name)}  ·  {size}  ·  {modified}\n{safe(entry.path)}\n{len(self.selected)}개 선택 · 마우스 드래그 이동 · c 복사 / x 잘라내기 / v 붙여넣기"))

    def toggle_mark(self, path: Path) -> None:
        if path in self.selected:
            self.selected.remove(path)
        else:
            self.selected.add(path)
        self.sync_marks()

    async def on_file_row_picked(self, message: FileRow.Picked) -> None:
        row = message.row
        if not 0 <= row.column < len(self.columns):
            return
        self.record("mouse", path=str(row.entry.path), context=message.context, double=message.double)
        self.active_column = row.column
        if message.context:
            if row.entry.path not in self.selected:
                self.selected = {row.entry.path}
            self.update_details(row.entry)
            self.sync_marks()
            self.run_worker(self.context(row.entry))
        elif message.shift:
            start = self.anchor[1] if self.anchor and self.anchor[0] == row.column else row.index
            self.selected = {entry.path for entry in self.columns[row.column].entries[min(start,row.index):max(start,row.index)+1]}
            self.sync_marks()
        elif message.multi:
            self.toggle_mark(row.entry.path)
            self.anchor = (row.column,row.index)
        else:
            self.anchor = (row.column,row.index)
            await self.activate(row.entry,row.column,toggle=True,open_file=message.double)

    async def activate(self, entry: Entry, column: int, *, toggle: bool = False, open_file: bool = False) -> None:
        async with self._navigation_lock:
            if not 0 <= column < len(self.columns) or not any(item.path == entry.path for item in self.columns[column].entries):
                return
            self.selected = {entry.path}
            self.active_column = column
            self.update_details(entry)
            self.columns[column].selected = entry.path
            if entry.is_dir:
                close = toggle and column+1 < len(self.columns) and self.columns[column+1].path == entry.path
                self.columns = self.columns[:column+1]
                self.columns[column].collapsed = close
                if not close:
                    child = await self.load_column(entry.path)
                    self.columns.append(child)
                await self.render_columns(column+1 if not close else column)
                self.record("navigate",path=str(entry.path),expanded=not close,columns=len(self.columns))
            else:
                self.columns = self.columns[:column+1]
                await self.render_columns(column)
                if open_file:
                    self.run_worker(self.open_external(entry.path))

    def chosen(self) -> list[Path]:
        if self.selected:
            return sorted(self.selected)
        view = self.query_one(f"#list-{self.active_column}",EntryList)
        return [view.current().entry.path] if view.current() else []

    async def open_external(self, path: Path) -> None:
        try:
            path = self.manager.validate(path)
            if os.name == "nt":
                os.startfile(path)
            else:
                command = ["/usr/bin/open",str(path)] if sys.platform == "darwin" else ["xdg-open",str(path)]
                result = await asyncio.to_thread(subprocess.run,command,capture_output=True,text=True,timeout=15)
                if result.returncode:
                    raise OSError(result.stderr.strip() or "기본 앱을 실행하지 못했습니다.")
            self.record("open",path=str(path))
        except (OSError,subprocess.TimeoutExpired) as error:
            self.notify(safe(error),severity="error")

    def action_copy(self) -> None:
        if paths := self.chosen():
            self.file_clipboard=(paths,True)
            self.notify(f"{len(paths)}개 복사 대기 · 대상 폴더로 이동 후 v")

    def action_cut(self) -> None:
        if paths := self.chosen():
            self.file_clipboard=(paths,False)
            self.notify(f"{len(paths)}개 이동 대기 · 대상 폴더로 이동 후 v")

    def action_paste(self) -> None:
        if self.file_clipboard:
            paths,copy=self.file_clipboard
            self.run_worker(self.transfer_to(paths,self.current_directory,copy=copy))
        else:
            self.notify("먼저 파일을 선택하고 c 또는 x를 누르세요.")

    def action_move(self) -> None:
        self.run_worker(self.move_dialog())

    async def move_dialog(self) -> None:
        paths=self.chosen()
        if not paths:
            return
        value=await self.push_screen_wait(InputDialog("이동할 폴더 경로",str(self.current_directory),"현재 탐색 루트 안의 폴더를 선택하세요."))
        if value:
            target=Path(value).expanduser()
            if not target.is_absolute():
                target=self.current_directory/target
            await self.transfer_to(paths,target,copy=False)

    async def transfer_to(self, paths: list[Path], destination: Path, *, copy: bool = False, external: bool = False) -> None:
        if self.busy:
            self.notify("진행 중인 파일 작업을 먼저 완료하세요.")
            return
        async with self._operation_lock:
            self.busy=True
            completed: list[Path]=[]
            try:
                destination=self.manager.validate(destination,directory=True)
                paths=list(dict.fromkeys(Path(path).absolute() for path in paths))
                paths=[source for source in paths if not any(source != parent and source.is_relative_to(parent) for parent in paths)]
                function=self.manager.import_external if external else self.manager.transfer
                # Collect all conflict decisions before changing any selected item.
                plans: list[tuple[Path,str,str | None]]=[]
                for source in paths:
                    name=None
                    policy="error"
                    while True:
                        target=destination/(name or source.name)
                        if target == source:
                            policy="skip"
                            break
                        if not os.path.lexists(target):
                            break
                        decision=await self.push_screen_wait(ConflictDialog(target))
                        if decision=="cancel":
                            return
                        if decision=="rename":
                            name=await self.push_screen_wait(InputDialog("다른 이름으로 저장",name or source.name))
                            if name is None:
                                return
                            continue
                        policy="overwrite" if decision=="overwrite" else "skip"
                        break
                    plans.append((source,policy,name))
                seen_targets=set()
                for source,policy,name in plans:
                    if policy=="skip":
                        continue
                    target=destination/(name or source.name)
                    key=str(target).casefold() if sys.platform in ("darwin","win32") else str(target)
                    if key in seen_targets:
                        raise ValueError("작업 대상의 이름이 중복됩니다. 다른 이름을 선택하세요.")
                    seen_targets.add(key)
                    for selected in paths:
                        if selected == target or selected.is_relative_to(target) or selected.resolve().is_relative_to(target.resolve()):
                            raise ValueError("선택한 원본 또는 원본을 포함하는 폴더를 덮어쓸 수 없습니다.")
                for source,policy,name in plans:
                    if policy=="skip":
                        continue
                    kwargs={"conflict":policy,"new_name":name}
                    if not external:
                        kwargs["copy"]=copy
                    completed.extend(await asyncio.to_thread(function,[source],destination,**kwargs))
                if not copy and not external:
                    self.file_clipboard=None
                self.selected=set(completed)
                self.record("copy" if copy or external else "move",sources=[str(path) for path in paths],destination=str(destination),completed=[str(path) for path in completed])
                self.notify(f"{len(completed)}개 {'복사' if copy or external else '이동'} 완료")
            except (OSError,ValueError) as error:
                self.record("error",operation="transfer",message=str(error))
                self.notify(f"{len(completed)}개 완료 후 중단: {safe(error)}",severity="error",timeout=8)
            finally:
                self.busy=False
                await self.refresh_columns()

    def action_rename(self) -> None:
        self.run_worker(self.rename_dialog())

    async def rename_dialog(self) -> None:
        paths=self.chosen()
        if len(paths)!=1:
            self.notify("이름을 바꿀 항목 한 개를 선택하세요.")
            return
        source=paths[0]
        value=await self.push_screen_wait(InputDialog("이름 변경",source.name))
        if value is None or value==source.name:
            return
        overwrite=False
        destination=source.parent/value
        if os.path.lexists(destination):
            choice=await self.push_screen_wait(ConflictDialog(destination))
            if choice=="rename":
                return await self.rename_dialog()
            if choice!="overwrite":
                return
            overwrite=True
        async with self._operation_lock:
            self.busy=True
            try:
                result=await asyncio.to_thread(self.manager.rename,source,value,overwrite)
                self.selected={result}
                self.record("rename",source=str(source),destination=str(result))
                self.notify("이름 변경 완료")
            except (OSError,ValueError) as error:
                self.notify(safe(error),severity="error")
            finally:
                self.busy=False
                await self.refresh_columns()

    def action_trash(self) -> None:
        self.run_worker(self.trash_dialog())

    async def trash_dialog(self) -> None:
        paths=self.chosen()
        if not paths:
            return
        prompt="선택한 항목을 휴지통으로 이동할까요?\n폴더는 하위 항목도 함께 이동합니다.\n\n"+"\n".join(safe(path.name) for path in paths[:5])
        if not await self.push_screen_wait(ConfirmDialog(prompt)):
            return
        async with self._operation_lock:
            self.busy=True
            try:
                await asyncio.to_thread(self.manager.trash,paths)
                self.record("trash",paths=[str(path) for path in paths])
                self.selected.clear()
                self.notify("휴지통으로 이동했습니다.")
            except (OSError,ValueError) as error:
                self.notify(safe(error),severity="error")
            finally:
                self.busy=False
                await self.refresh_columns()

    def action_import(self) -> None:
        self.run_worker(self.import_dialog())

    async def import_dialog(self, text: str | None = None) -> None:
        if text is None:
            text=await self.push_screen_wait(InputDialog("외부 파일 가져오기",hint="파일을 이 입력 칸에 드래그하거나 경로를 붙여넣으세요. 원본은 유지됩니다."))
        if not text:
            return
        try:
            paths=parse_paths(text,self.current_directory)
            destination=self.current_directory
            if await self.push_screen_wait(ConfirmDialog(f"외부 {len(paths)}개 항목을 여기에 복사할까요?\n{safe(destination)}\n원본은 그대로 유지됩니다.")):
                await self.transfer_to(paths,destination,copy=True,external=True)
        except (OSError,ValueError) as error:
            self.notify(safe(error),severity="error")

    def on_paste(self, event: events.Paste) -> None:
        if len(self.screen_stack)==1:
            event.stop()
            self.run_worker(self.import_dialog(event.text))

    def action_context(self) -> None:
        view=self.query_one(f"#list-{self.active_column}",EntryList)
        if row:=view.current():
            if row.entry.path not in self.selected:
                self.selected={row.entry.path}
            self.run_worker(self.context(row.entry))

    async def context(self, entry: Entry) -> None:
        action=await self.push_screen_wait(ContextDialog(entry.name))
        if action=="open":
            await self.activate(entry,self.active_column,toggle=False,open_file=True)
        elif action=="copy":
            self.action_copy()
        elif action=="cut":
            self.action_cut()
        elif action=="paste" and self.file_clipboard:
            sources,copy=self.file_clipboard
            await self.transfer_to(sources,entry.path if entry.is_dir else entry.path.parent,copy=copy)
        elif action=="move":
            await self.move_dialog()
        elif action=="rename":
            await self.rename_dialog()
        elif action=="trash":
            await self.trash_dialog()
        elif action=="import":
            await self.import_dialog()

    def action_select_all(self) -> None:
        self.selected={entry.path for entry in self.columns[self.active_column].entries}
        self.sync_marks()

    def action_refresh(self) -> None:
        self.run_worker(self.refresh_columns())

    def action_hidden(self) -> None:
        self.show_hidden=not self.show_hidden
        self.query_one("#hidden",Button).label="점 파일 ON" if self.show_hidden else "점 파일 OFF"
        self.run_worker(self.refresh_columns())

    def action_parent(self) -> None:
        self.run_worker(self.change_root(self.manager.root.parent))

    def action_location(self) -> None:
        self.run_worker(self.location_dialog())

    async def location_dialog(self) -> None:
        value=await self.push_screen_wait(InputDialog("탐색할 폴더",str(self.current_directory)))
        if value:
            await self.change_root(Path(value).expanduser())

    async def change_root(self, path: Path) -> None:
        try:
            manager=FileManager(path)
            manager.validate(manager.root,directory=True)
            self.manager=manager
            self.columns=[]
            self.selected.clear()
            self.active_column=0
            await self.refresh_columns()
        except (OSError,ValueError) as error:
            self.notify(safe(error),severity="error")

    def action_close_browser(self) -> None:
        if len(self.screen_stack)==1:
            self.exit()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        actions={"parent":self.action_parent,"refresh":self.action_refresh,"hidden":self.action_hidden}
        if event.button.id in actions:
            actions[event.button.id]()

    def on_select_changed(self, event: Select.Changed) -> None:
        if self.browser_ready and event.select.id=="sort" and event.value in ("folders","name","recent"):
            self.sort_order=str(event.value)
            self.run_worker(self.refresh_columns())
