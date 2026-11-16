//! Read-only SSH browser. Network jobs run outside the input/render loop.
use crate::{
    app::{clean, entry_icon, launch_downloaded},
    fs::{Entry, Sort},
    ssh::{Listing, Remote, Task},
};
use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct Column {
    listing: Listing,
    entries: Vec<Entry>,
    state: ListState,
    via: Option<PathBuf>,
}
impl Column {
    fn new(listing: Listing, hidden: bool, sort: Sort, via: Option<PathBuf>) -> Self {
        let entries = listing.sorted(hidden, sort);
        let selected = if entries.is_empty() { None } else { Some(0) };
        Self {
            listing,
            entries,
            state: ListState::default().with_selected(selected),
            via,
        }
    }
    fn reorder(&mut self, hidden: bool, sort: Sort) {
        let old = self.current().map(|e| e.path.clone());
        self.entries = self.listing.sorted(hidden, sort);
        let n = old
            .and_then(|p| self.entries.iter().position(|e| e.path == p))
            .or(if self.entries.is_empty() {
                None
            } else {
                Some(0)
            });
        self.state.select(n);
    }
    fn current(&self) -> Option<&Entry> {
        self.entries.get(self.state.selected()?)
    }
}
#[derive(Clone, Debug)]
enum Place {
    Root,
    Child {
        parent: usize,
        via: PathBuf,
        focus: bool,
    },
    Refresh(usize),
}
enum Action {
    List(PathBuf, Place),
    Open(PathBuf),
    Cancel,
    Quit,
}
enum Dialog {
    Location(String),
    Menu(usize),
}
struct Browser {
    target: String,
    columns: Vec<Column>,
    active: usize,
    hidden: bool,
    sort: Sort,
    status: String,
    areas: Vec<(usize, Rect)>,
    buttons: Vec<(Rect, char)>,
    dialog: Option<Dialog>,
    dialog_area: Rect,
    last_click: Option<(PathBuf, Instant)>,
    pressed: Option<(usize, PathBuf)>,
    busy: bool,
}
impl Browser {
    fn new(target: String, listing: Listing) -> Self {
        Self {
            target,
            columns: vec![Column::new(listing, true, Sort::Folders, None)],
            active: 0,
            hidden: true,
            sort: Sort::Folders,
            status: "SSH 읽기 전용 · 파일은 내려받아 로컬 앱으로 열립니다".into(),
            areas: vec![],
            buttons: vec![],
            dialog: None,
            dialog_area: Rect::default(),
            last_click: None,
            pressed: None,
            busy: false,
        }
    }
    fn current(&self) -> Option<&Entry> {
        self.columns.get(self.active)?.current()
    }
    fn directory(&self) -> PathBuf {
        if let Some(child) = self.columns.get(self.active + 1) {
            if self
                .current()
                .is_some_and(|e| child.via.as_ref() == Some(&e.path))
            {
                return child.listing.path.clone();
            }
        }
        self.columns[self.active].listing.path.clone()
    }
    fn activate(&mut self, toggle: bool, open: bool, focus: bool) -> Option<Action> {
        let e = self.current()?.clone();
        if e.is_dir {
            let close = toggle
                && self
                    .columns
                    .get(self.active + 1)
                    .is_some_and(|c| c.via.as_ref() == Some(&e.path));
            self.columns.truncate(self.active + 1);
            if close {
                return Some(Action::Cancel);
            }
            Some(Action::List(
                e.path.clone(),
                Place::Child {
                    parent: self.active,
                    via: e.path,
                    focus,
                },
            ))
        } else {
            self.columns.truncate(self.active + 1);
            if open {
                Some(Action::Open(e.path))
            } else {
                None
            }
        }
    }
    fn apply(&mut self, listing: Listing, place: Place) {
        match place {
            Place::Root => {
                self.columns = vec![Column::new(listing, self.hidden, self.sort, None)];
                self.active = 0;
            }
            Place::Child { parent, via, focus } => {
                if parent >= self.columns.len()
                    || !self.columns[parent]
                        .current()
                        .is_some_and(|e| e.path == via)
                {
                    return;
                }
                self.columns.truncate(parent + 1);
                self.columns
                    .push(Column::new(listing, self.hidden, self.sort, Some(via)));
                if focus {
                    self.active = parent + 1;
                }
            }
            Place::Refresh(i) => {
                if i < self.columns.len() {
                    let old = self.columns[i].current().map(|e| e.path.clone());
                    let via = self.columns[i].via.clone();
                    let mut col = Column::new(listing, self.hidden, self.sort, via);
                    if let Some(n) = old.and_then(|p| col.entries.iter().position(|e| e.path == p))
                    {
                        col.state.select(Some(n));
                    }
                    self.columns.truncate(i + 1);
                    self.columns[i] = col;
                    self.active = self.active.min(i);
                }
            }
        }
        self.status = "원격 목록을 불러왔습니다".into();
    }
    fn command(&mut self, ch: char) -> Option<Action> {
        match ch {
            'q' => Some(Action::Quit),
            'o' => self.activate(false, true, false),
            'u' => {
                self.dialog = Some(Dialog::Menu(0));
                None
            }
            'p' => Some(Action::List(
                self.directory()
                    .parent()
                    .unwrap_or(Path::new("/"))
                    .to_path_buf(),
                Place::Root,
            )),
            'f' => {
                let index = if self.columns.get(self.active + 1).is_some_and(|c| {
                    self.current()
                        .is_some_and(|e| c.via.as_ref() == Some(&e.path))
                }) {
                    self.active + 1
                } else {
                    self.active
                };
                Some(Action::List(
                    self.columns[index].listing.path.clone(),
                    Place::Refresh(index),
                ))
            }
            'h' | 's' => {
                if ch == 'h' {
                    self.hidden = !self.hidden;
                } else {
                    self.sort = match self.sort {
                        Sort::Folders => Sort::Name,
                        Sort::Name => Sort::Recent,
                        Sort::Recent => Sort::Folders,
                    };
                }
                for col in &mut self.columns {
                    col.reorder(self.hidden, self.sort);
                }
                for i in 1..self.columns.len() {
                    if !self.columns[i - 1]
                        .entries
                        .iter()
                        .any(|e| Some(&e.path) == self.columns[i].via.as_ref())
                    {
                        self.columns.truncate(i);
                        break;
                    }
                }
                self.active = self.active.min(self.columns.len() - 1);
                None
            }
            'l' => {
                self.dialog = Some(Dialog::Location(self.directory().to_string_lossy().into()));
                None
            }
            'c' => Some(Action::Cancel),
            _ => None,
        }
    }
    fn key(&mut self, k: KeyEvent) -> Option<Action> {
        if k.kind == KeyEventKind::Release {
            return None;
        }
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('q') {
            return Some(Action::Quit);
        }
        if let Some(dialog) = self.dialog.take() {
            match dialog {
                Dialog::Location(mut text) => {
                    match k.code {
                        KeyCode::Esc => return None,
                        KeyCode::Enter => {
                            let p = PathBuf::from(&text);
                            let p = if p.is_absolute() || text.starts_with('~') {
                                p
                            } else {
                                self.directory().join(p)
                            };
                            return Some(Action::List(p, Place::Root));
                        }
                        KeyCode::Backspace => {
                            text.pop();
                        }
                        KeyCode::Char('a') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                            text.clear()
                        }
                        KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                            text.push(c)
                        }
                        _ => {}
                    }
                    self.dialog = Some(Dialog::Location(text));
                }
                Dialog::Menu(mut index) => {
                    match k.code {
                        KeyCode::Esc => return None,
                        KeyCode::Char('q') => return Some(Action::Quit),
                        KeyCode::Up => index = (index + 3) % 4,
                        KeyCode::Down => index = (index + 1) % 4,
                        KeyCode::Enter => return self.command(['o', 'f', 'h', 's'][index]),
                        _ => {}
                    }
                    self.dialog = Some(Dialog::Menu(index));
                }
            }
            return None;
        }
        match k.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Esc => Some(Action::Cancel),
            KeyCode::Enter => self.activate(false, true, false),
            KeyCode::Right => self.activate(false, false, true),
            KeyCode::Left => {
                self.active = self.active.saturating_sub(1);
                Some(Action::Cancel)
            }
            KeyCode::Backspace => self.command('p'),
            KeyCode::F(5) => self.command('f'),
            KeyCode::F(10) => self.command('u'),
            KeyCode::Up | KeyCode::Down => {
                let col = &mut self.columns[self.active];
                if !col.entries.is_empty() {
                    let n = col.state.selected().unwrap_or(0);
                    col.state.select(Some(if k.code == KeyCode::Up {
                        n.saturating_sub(1)
                    } else {
                        (n + 1).min(col.entries.len() - 1)
                    }));
                }
                None
            }
            KeyCode::Char('l') if k.modifiers.contains(KeyModifiers::CONTROL) => self.command('l'),
            KeyCode::Char(c) if k.modifiers.is_empty() => self.command(c),
            _ => None,
        }
    }
    fn hit(&self, x: u16, y: u16) -> Option<(usize, usize)> {
        self.areas.iter().find_map(|(i, r)| {
            if x > r.x
                && x < r.right().saturating_sub(1)
                && y > r.y
                && y < r.bottom().saturating_sub(1)
            {
                let n = self.columns[*i].state.offset() + usize::from(y - r.y - 1);
                if n < self.columns[*i].entries.len() {
                    return Some((*i, n));
                }
            }
            None
        })
    }
    fn mouse(&mut self, m: MouseEvent) -> Option<Action> {
        if self.dialog.is_some() {
            if m.kind == MouseEventKind::Down(MouseButton::Left) {
                if !self.dialog_area.contains((m.column, m.row).into()) {
                    if matches!(self.dialog, Some(Dialog::Menu(_))) {
                        self.dialog = None;
                    }
                    return None;
                }
                if matches!(self.dialog, Some(Dialog::Menu(_))) {
                    let n = usize::from(m.row.saturating_sub(self.dialog_area.y + 1));
                    if n < 4 {
                        self.dialog = None;
                        return self.command(['o', 'f', 'h', 's'][n]);
                    }
                }
            }
            return None;
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((_, ch)) = self
                    .buttons
                    .iter()
                    .find(|(r, _)| r.contains((m.column, m.row).into()))
                {
                    return self.command(*ch);
                }
                if let Some((i, n)) = self.hit(m.column, m.row) {
                    self.active = i;
                    self.columns[i].state.select(Some(n));
                    self.pressed = Some((i, self.columns[i].entries[n].path.clone()));
                }
            }
            MouseEventKind::Drag(_) => self.pressed = None,
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some((i, path)) = self.pressed.take() {
                    if self.hit(m.column, m.row).is_some_and(|(col, n)| {
                        col == i && self.columns[col].entries[n].path == path
                    }) {
                        self.active = i;
                        let folder = self.current().is_some_and(|e| e.is_dir);
                        let double = !folder
                            && self.last_click.as_ref().is_some_and(|(p, t)| {
                                p == &path && t.elapsed() < Duration::from_millis(550)
                            });
                        self.last_click = if folder || double {
                            None
                        } else {
                            Some((path, Instant::now()))
                        };
                        return self.activate(folder, double, false);
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some((i, n)) = self.hit(m.column, m.row) {
                    self.active = i;
                    self.columns[i].state.select(Some(n));
                    self.dialog = Some(Dialog::Menu(0));
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                if let Some((i, _)) = self
                    .areas
                    .iter()
                    .find(|(_, r)| r.contains((m.column, m.row).into()))
                {
                    self.active = *i;
                    let col = &mut self.columns[*i];
                    if !col.entries.is_empty() {
                        let n = col.state.selected().unwrap_or(0);
                        col.state
                            .select(Some(if m.kind == MouseEventKind::ScrollDown {
                                (n + 3).min(col.entries.len() - 1)
                            } else {
                                n.saturating_sub(3)
                            }));
                    }
                }
            }
            _ => {}
        }
        None
    }
    fn draw(&mut self, f: &mut Frame) {
        let a = f.area();
        if a.width < 30 || a.height < 10 {
            f.render_widget(Paragraph::new("터미널을 넓혀주세요 · q 종료"), a);
            return;
        }
        f.render_widget(
            Paragraph::new(format!(
                " FinderViewer SSH · {} · 읽기 전용",
                clean(&self.target)
            ))
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Rect::new(a.x, a.y, a.width, 1),
        );
        f.render_widget(
            Paragraph::new(format!(" {}", clean(self.directory().display()))),
            Rect::new(a.x, a.y + 1, a.width, 1),
        );
        self.buttons.clear();
        let mut x = a.x;
        for (label, ch) in [
            ("상위", 'p'),
            ("새로고침", 'f'),
            (
                if self.hidden {
                    "숨김 ON"
                } else {
                    "숨김 OFF"
                },
                'h',
            ),
            (
                match self.sort {
                    Sort::Folders => "폴더 우선",
                    Sort::Name => "이름순",
                    Sort::Recent => "최근순",
                },
                's',
            ),
        ] {
            let width = Line::from(label).width() as u16 + 2;
            if x + width > a.right() {
                break;
            }
            let r = Rect::new(x, a.y + 2, width, 1);
            f.render_widget(
                Paragraph::new(format!(" {label} "))
                    .style(Style::default().fg(Color::Black).bg(Color::Gray)),
                r,
            );
            self.buttons.push((r, ch));
            x += width + 1;
        }
        let content = Rect::new(a.x, a.y + 3, a.width, a.height - 8);
        let visible = usize::from((a.width / 28).max(1));
        let reveal = (self.active + 1).min(self.columns.len() - 1);
        let start = reveal.saturating_sub(visible - 1);
        let width = if visible == 1 {
            a.width
        } else {
            a.width / visible as u16
        };
        self.areas.clear();
        for (offset, index) in (start..self.columns.len()).take(visible).enumerate() {
            let r = Rect::new(
                content.x + offset as u16 * width,
                content.y,
                width,
                content.height,
            );
            self.areas.push((index, r));
            let col = &mut self.columns[index];
            let title = col
                .listing
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into());
            let block = Block::default()
                .borders(Borders::ALL)
                .title(clean(title))
                .border_style(Style::default().fg(if index == self.active {
                    Color::Cyan
                } else {
                    Color::DarkGray
                }));
            if col.entries.is_empty() {
                f.render_widget(Paragraph::new("빈 폴더").block(block), r);
            } else {
                let items: Vec<ListItem> = col
                    .entries
                    .iter()
                    .map(|e| {
                        ListItem::new(Line::from(vec![
                            Span::raw(format!(" {} ", entry_icon(e))),
                            Span::styled(
                                clean(&e.name),
                                if e.is_dir {
                                    Style::default().fg(Color::Yellow)
                                } else {
                                    Style::default()
                                },
                            ),
                        ]))
                    })
                    .collect();
                f.render_stateful_widget(
                    List::new(items).block(block).highlight_style(
                        Style::default()
                            .bg(if index == self.active {
                                Color::Blue
                            } else {
                                Color::DarkGray
                            })
                            .fg(Color::LightGreen),
                    ),
                    r,
                    &mut col.state,
                );
            }
        }
        let y = a.bottom() - 5;
        let lines = if let Some(e) = self.current() {
            vec![
                Line::from(vec![
                    Span::styled(
                        format!(" {}", clean(&e.name)),
                        Style::default()
                            .fg(Color::LightGreen)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(if e.is_dir {
                        " · 폴더".into()
                    } else {
                        format!(" · {} bytes", e.size)
                    }),
                ]),
                Line::from(format!(" {}", clean(e.path.display()))),
            ]
        } else {
            vec![Line::from(" 선택한 항목 없음")]
        };
        f.render_widget(Paragraph::new(lines), Rect::new(a.x, y, a.width, 2));
        let mut x = a.x;
        for (label, ch) in [
            ("열기", 'o'),
            ("메뉴", 'u'),
            ("경로", 'l'),
            ("취소", 'c'),
            ("종료", 'q'),
        ] {
            let w = Line::from(label).width() as u16 + 2;
            if x + w > a.right() {
                break;
            }
            let r = Rect::new(x, y + 2, w, 1);
            f.render_widget(
                Paragraph::new(format!(" {label} ")).style(Style::default().fg(Color::White).bg(
                    if ch == 'o' {
                        Color::Blue
                    } else {
                        Color::DarkGray
                    },
                )),
                r,
            );
            self.buttons.push((r, ch));
            x += w + u16::from(a.width >= 35);
        }
        f.render_widget(
            Paragraph::new(format!(
                " {}{}",
                if self.busy { "통신 중 · " } else { "" },
                clean(&self.status)
            ))
            .style(Style::default().fg(Color::Yellow)),
            Rect::new(a.x, y + 3, a.width, 1),
        );
        f.render_widget(
            Paragraph::new(" 원격 수정·업로드 없음 · 파일 열기는 로컬 복사본 · q 종료")
                .style(Style::default().fg(Color::DarkGray)),
            Rect::new(a.x, y + 4, a.width, 1),
        );
        if let Some(dialog) = &self.dialog {
            let w = a.width.saturating_sub(4).min(72);
            let h = 8.min(a.height - 2);
            let r = Rect::new(a.x + (a.width - w) / 2, a.y + (a.height - h) / 2, w, h);
            self.dialog_area = r;
            f.render_widget(Clear, r);
            match dialog {
                Dialog::Location(text) => {
                    f.render_widget(
                        Paragraph::new(vec![
                            Line::from(clean(text)),
                            Line::from(""),
                            Line::from("Enter 확인 · Esc 취소 · Ctrl+A 비우기"),
                        ])
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title("원격 폴더 경로"),
                        ),
                        r,
                    );
                }
                Dialog::Menu(n) => {
                    let mut state = ListState::default().with_selected(Some(*n));
                    f.render_stateful_widget(
                        List::new([
                            "열기 (로컬 복사본)",
                            "새로고침",
                            "숨김 파일 토글",
                            "정렬 변경",
                        ])
                        .block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title("SSH 읽기 전용 메뉴"),
                        )
                        .highlight_style(Style::default().bg(Color::Blue)),
                        r,
                        &mut state,
                    );
                }
            }
        }
    }
}

pub fn run(target: String, path: PathBuf) -> Result<()> {
    // Authentication uses OpenSSH's normal terminal prompts before raw mode.
    let (remote, listing) = Remote::connect(target, &path)?;
    let mut browser = Browser::new(remote.target().into(), listing);
    let (guard, mut terminal) = crate::app::terminal()?;
    let mut navigation: Option<(Task<Listing>, Place)> = None;
    let mut download: Option<Task<PathBuf>> = None;
    let mut opening: Option<Task<()>> = None;
    let result = (|| -> Result<()> {
        let mut dirty = true;
        loop {
            if let Some((task, _)) = &navigation {
                if let Some(result) = task.poll() {
                    let (_, place) = navigation.take().unwrap();
                    match result {
                        Ok(listing) => browser.apply(listing, place),
                        Err(e) => browser.status = format!("목록 오류: {e}"),
                    };
                    dirty = true;
                }
            }
            if let Some(task) = &download {
                if let Some(result) = task.poll() {
                    download = None;
                    match result {
                        Ok(local) => {
                            browser.status = format!("로컬 복사본: {}", clean(local.display()));
                            opening = Some(Task::spawn(move |cancel| {
                                if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                    launch_downloaded(&local)?;
                                }
                                Ok(())
                            }));
                        }
                        Err(e) => browser.status = format!("내려받기 오류: {e}"),
                    }
                    dirty = true;
                }
            }
            if let Some(task) = &opening {
                if let Some(result) = task.poll() {
                    opening = None;
                    if let Err(e) = result {
                        browser.status = format!("로컬 앱 열기 실패: {e}");
                    }
                    dirty = true;
                }
            }
            browser.busy = navigation.is_some() || download.is_some() || opening.is_some();
            if dirty {
                terminal.draw(|f| browser.draw(f))?;
                dirty = false;
            }
            if !event::poll(Duration::from_millis(50))? {
                continue;
            }
            dirty = true;
            let action = match event::read()? {
                Event::Key(key) => browser.key(key),
                Event::Mouse(mouse) => browser.mouse(mouse),
                Event::Paste(text) => {
                    if let Some(Dialog::Location(value)) = &mut browser.dialog {
                        value.push_str(&text);
                    }
                    None
                }
                _ => None,
            };
            match action {
                Some(Action::Quit) => break,
                Some(Action::Cancel) => {
                    navigation = None;
                    download = None;
                    opening = None;
                    browser.status = "작업을 취소했습니다".into();
                }
                Some(Action::List(path, place)) => {
                    let r = remote.clone();
                    navigation = Some((Task::spawn(move |cancel| r.list(&path, cancel)), place));
                    browser.status = "원격 폴더 목록을 불러오는 중… · q 즉시 종료".into();
                }
                Some(Action::Open(path)) => {
                    let r = remote.clone();
                    download = Some(Task::spawn(move |cancel| r.download(&path, cancel)));
                    browser.status = "로컬 복사본을 내려받는 중… · 취소/Esc 또는 q".into();
                }
                None => {}
            }
        }
        Ok(())
    })();
    // Signal cancellation before restoring the terminal. Control socket shutdown
    // and remaining process cleanup never hold up q or terminal restoration.
    drop(navigation);
    drop(download);
    drop(opening);
    drop(terminal);
    drop(guard);
    remote.close();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;
    fn entry(path: &str, dir: bool) -> Entry {
        Entry {
            path: path.into(),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into(),
            is_dir: dir,
            is_symlink: false,
            size: 12,
            modified: UNIX_EPOCH,
        }
    }
    fn listing(path: &str, entries: Vec<Entry>) -> Listing {
        Listing {
            path: path.into(),
            entries,
        }
    }
    fn browser() -> Browser {
        Browser::new(
            "test-host".into(),
            listing(
                "/remote",
                vec![
                    entry("/remote/folder", true),
                    entry("/remote/한글 file.py", false),
                    entry("/remote/picture.png", false),
                ],
            ),
        )
    }
    fn render(b: &mut Browser) {
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        t.draw(|f| b.draw(f)).unwrap();
    }
    fn click(b: &mut Browser, x: u16, y: u16) -> Option<Action> {
        b.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        b.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        })
    }
    #[test]
    fn folder_request_is_async_and_second_click_closes_canonical_symlink_column() {
        let mut b = browser();
        let action = b.activate(true, false, false).unwrap();
        let Action::List(path, place) = action else {
            panic!()
        };
        assert_eq!(path, Path::new("/remote/folder"));
        assert_eq!(b.columns.len(), 1);
        b.apply(listing("/actual/canonical", vec![]), place);
        assert_eq!(b.columns.len(), 2);
        assert!(matches!(
            b.activate(true, false, false),
            Some(Action::Cancel)
        ));
        assert_eq!(b.columns.len(), 1);
    }
    #[test]
    fn stale_child_result_cannot_replace_a_new_parent_selection() {
        let mut b = browser();
        let Some(Action::List(_, place)) = b.activate(false, false, false) else {
            panic!()
        };
        b.columns[0].state.select(Some(1));
        b.apply(listing("/remote/folder", vec![]), place);
        assert_eq!(b.columns.len(), 1);
    }
    #[test]
    fn file_single_click_only_selects_and_doubleclick_requests_local_copy() {
        let mut b = browser();
        render(&mut b);
        let r = b.areas[0].1;
        let x = r.x + 2;
        let index = b.columns[0]
            .entries
            .iter()
            .position(|e| e.name == "한글 file.py")
            .unwrap();
        let y = r.y + 1 + index as u16;
        assert!(click(&mut b, x, y).is_none());
        assert_eq!(b.current().unwrap().name, "한글 file.py");
        let Some(Action::Open(path)) = click(&mut b, x, y) else {
            panic!()
        };
        assert_eq!(path, Path::new("/remote/한글 file.py"));
    }
    #[test]
    fn quit_and_cancel_remain_available_while_remote_work_is_busy() {
        let mut b = browser();
        b.busy = true;
        assert!(matches!(
            b.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Action::Quit)
        ));
        assert!(matches!(
            b.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Action::Cancel)
        ));
        b.dialog = Some(Dialog::Location("/remote".into()));
        assert!(matches!(
            b.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        ));
    }
    #[test]
    fn location_input_preserves_unicode_q_and_relative_paths() {
        let mut b = browser();
        b.dialog = Some(Dialog::Location(String::new()));
        for c in "한글 q".chars() {
            assert!(b
                .key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
                .is_none());
        }
        let Some(Action::List(path, Place::Root)) =
            b.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        else {
            panic!()
        };
        assert_eq!(path, Path::new("/remote/한글 q"));
    }
    #[test]
    fn read_only_footer_has_no_mutation_actions_and_quit_is_mouse_clickable() {
        let mut b = browser();
        b.busy = true;
        render(&mut b);
        assert!(!b.buttons.iter().any(|(_, c)| "dxvimr".contains(*c)));
        let r = b.buttons.iter().find(|(_, c)| *c == 'q').unwrap().0;
        assert!(matches!(
            b.mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: r.x + 1,
                row: r.y,
                modifiers: KeyModifiers::NONE
            }),
            Some(Action::Quit)
        ));
    }
    #[test]
    fn menu_dismisses_on_background_and_shows_only_read_actions() {
        let mut b = browser();
        b.command('u');
        render(&mut b);
        b.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        assert!(b.dialog.is_none());
    }
    #[test]
    fn sorting_and_hidden_filter_do_not_request_network() {
        let mut b = Browser::new(
            "host".into(),
            listing(
                "/remote",
                vec![entry("/remote/.hidden", false), entry("/remote/z", true)],
            ),
        );
        assert!(b.command('h').is_none());
        assert_eq!(b.columns[0].entries.len(), 1);
        assert!(b.command('s').is_none());
    }
}
