use crate::fs::{Entry, Manager, Policy, Sort};
use anyhow::{bail, Result};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};

struct Column {
    path: PathBuf,
    entries: Vec<Entry>,
    state: ListState,
}
#[derive(Clone)]
struct Transfer {
    sources: Vec<PathBuf>,
    dest: PathBuf,
    copy: bool,
    external: bool,
    index: usize,
    completed: usize,
    name: Option<String>,
    policy: Policy,
}
#[derive(Clone)]
enum InputKind {
    Location,
    Move,
    Import,
    Rename(PathBuf),
    ConflictName(Transfer),
}
enum Dialog {
    Input {
        kind: InputKind,
        title: String,
        text: String,
        cursor: usize,
    },
    Confirm(Vec<PathBuf>),
    ImportConfirm(Vec<PathBuf>),
    Conflict(Transfer),
    RenameConflict(PathBuf, String),
    Menu(usize),
}
struct App {
    manager: Manager,
    columns: Vec<Column>,
    active: usize,
    selected: BTreeSet<PathBuf>,
    anchor: Option<(usize, usize)>,
    hidden: bool,
    sort: Sort,
    clipboard: Option<(Vec<PathBuf>, bool)>,
    dialog: Option<Dialog>,
    status: String,
    quit: bool,
    areas: Vec<(usize, Rect)>,
    open_button: Rect,
    dialog_area: Rect,
    dialog_buttons: Vec<(Rect, KeyCode)>,
    toolbar: Vec<(Rect, char)>,
    last_click: Option<(PathBuf, Instant)>,
    drag: Option<(usize, PathBuf, bool)>,
    dragged: bool,
    job: Option<Receiver<Result<String>>>,
    pending_transfer: Option<Transfer>,
    last_refresh: Instant,
    opener: fn(&Path) -> Result<()>,
}
fn clean(value: impl std::fmt::Display) -> String {
    value
        .to_string()
        .chars()
        .map(|c| if c.is_control() { '�' } else { c })
        .collect()
}
fn expand(text: &str, base: &Path) -> PathBuf {
    let p = if text == "~" {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
    } else if let Some(s) = text.strip_prefix("~/") {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(s)
    } else {
        PathBuf::from(text)
    };
    if p.is_absolute() {
        p
    } else {
        base.join(p)
    }
}
fn paths(text: &str, base: &Path) -> Result<Vec<PathBuf>> {
    let values = shell_words::split(text)?;
    if values.is_empty() {
        bail!("경로를 입력하세요.")
    };
    values
        .into_iter()
        .map(|s| {
            if let Some(rest) = s.strip_prefix("file://") {
                let path = rest.strip_prefix("localhost").unwrap_or(rest);
                if !path.starts_with('/') {
                    bail!("로컬 file:// 경로만 가져올 수 있습니다.");
                }
                return Ok(PathBuf::from(
                    percent_encoding::percent_decode_str(path)
                        .decode_utf8()?
                        .as_ref(),
                ));
            }
            Ok(expand(&s, base))
        })
        .collect()
}
fn launch_external(path: &Path) -> Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("/usr/bin/open")
    } else if cfg!(target_os = "windows") {
        Command::new("explorer.exe")
    } else {
        Command::new("xdg-open")
    };
    let mut child = command
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("기본 앱 열기에 실패했습니다 ({status}). 파일 연결을 확인하세요.");
            }
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("기본 앱 응답 시간이 초과됐습니다.");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
impl App {
    fn new(root: PathBuf) -> Result<Self> {
        let manager = Manager::new(root)?;
        let mut app = Self {
            manager,
            columns: vec![],
            active: 0,
            selected: BTreeSet::new(),
            anchor: None,
            hidden: true,
            sort: Sort::Folders,
            clipboard: None,
            dialog: None,
            status: "파일 선택 후 하단 열기 · Enter/o · F10 메뉴 · q 종료".into(),
            quit: false,
            areas: vec![],
            open_button: Rect::default(),
            dialog_area: Rect::default(),
            dialog_buttons: vec![],
            toolbar: vec![],
            last_click: None,
            drag: None,
            dragged: false,
            job: None,
            pending_transfer: None,
            last_refresh: Instant::now(),
            opener: launch_external,
        };
        app.refresh()?;
        Ok(app)
    }
    fn column(&self, path: PathBuf) -> Result<Column> {
        let entries = self.manager.list(&path, self.hidden, self.sort)?;
        let mut state = ListState::default();
        if !entries.is_empty() {
            state.select(Some(0));
        }
        Ok(Column {
            path,
            entries,
            state,
        })
    }
    fn current(&self) -> Option<Entry> {
        let col = self.columns.get(self.active)?;
        col.entries.get(col.state.selected()?).cloned()
    }
    fn directory(&self) -> PathBuf {
        if let Some(child) = self.columns.get(self.active + 1) {
            if self
                .current()
                .is_some_and(|e| e.is_dir && e.path == child.path)
            {
                return child.path.clone();
            }
        }
        self.columns
            .get(self.active)
            .map(|c| c.path.clone())
            .unwrap_or_else(|| self.manager.root.clone())
    }
    fn chosen(&self) -> Vec<PathBuf> {
        if self.selected.is_empty() {
            self.current().map(|e| vec![e.path]).unwrap_or_default()
        } else {
            self.selected.iter().cloned().collect()
        }
    }
    fn refresh(&mut self) -> Result<()> {
        if self.columns.is_empty() {
            self.columns.push(self.column(self.manager.root.clone())?);
        } else {
            let mut valid = 0;
            for i in 0..self.columns.len() {
                let p = self.columns[i].path.clone();
                if i > 0
                    && !self.columns[i - 1]
                        .entries
                        .iter()
                        .any(|e| e.is_dir && e.path == p)
                {
                    break;
                }
                match self.manager.list(&p, self.hidden, self.sort) {
                    Ok(entries) => {
                        let old = self.columns[i]
                            .state
                            .selected()
                            .and_then(|n| self.columns[i].entries.get(n))
                            .map(|e| e.path.clone());
                        let index = old
                            .and_then(|p| entries.iter().position(|e| e.path == p))
                            .or(if entries.is_empty() { None } else { Some(0) });
                        self.columns[i].entries = entries;
                        self.columns[i].state.select(index);
                        valid += 1;
                    }
                    Err(e) => {
                        if i == 0 {
                            return Err(e);
                        }
                        break;
                    }
                }
            }
            self.columns.truncate(valid);
        }
        self.active = self.active.min(self.columns.len().saturating_sub(1));
        if self
            .anchor
            .is_some_and(|(c, i)| self.columns.get(c).is_none_or(|col| i >= col.entries.len()))
        {
            self.anchor = None;
        }
        self.selected
            .retain(|p| std::fs::symlink_metadata(p).is_ok());
        self.last_refresh = Instant::now();
        Ok(())
    }
    fn change_root(&mut self, path: PathBuf) -> Result<()> {
        let manager = Manager::new(path)?;
        let entries = manager.list(&manager.root, self.hidden, self.sort)?;
        let mut state = ListState::default();
        if !entries.is_empty() {
            state.select(Some(0));
        }
        self.manager = manager;
        self.columns = vec![Column {
            path: self.manager.root.clone(),
            entries,
            state,
        }];
        self.active = 0;
        self.selected.clear();
        self.anchor = None;
        Ok(())
    }
    fn activate(&mut self, toggle: bool, open: bool) -> Result<()> {
        if let Some(entry) = self.current() {
            self.selected = BTreeSet::from([entry.path.clone()]);
            if entry.is_dir {
                let close = toggle
                    && self
                        .columns
                        .get(self.active + 1)
                        .is_some_and(|c| c.path == entry.path);
                self.columns.truncate(self.active + 1);
                let child = if close {
                    None
                } else {
                    Some(self.column(entry.path)?)
                };
                self.columns.truncate(self.active + 1);
                if let Some(child) = child {
                    self.columns.push(child);
                }
            } else {
                self.columns.truncate(self.active + 1);
                if open {
                    self.open_file(entry.path)?;
                }
            }
        }
        Ok(())
    }
    fn open_file(&mut self, path: PathBuf) -> Result<()> {
        let path = self.manager.validate(&path, false)?;
        let opener = self.opener;
        self.spawn(move || {
            opener(&path)?;
            Ok(format!(
                "기본 앱으로 열었습니다: {}",
                clean(path.file_name().unwrap_or_default().to_string_lossy())
            ))
        });
        Ok(())
    }
    fn spawn(&mut self, job: impl FnOnce() -> Result<String> + Send + 'static) {
        if self.job.is_some() {
            self.status = "파일 작업이 진행 중입니다.".into();
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        self.status = "처리 중…".into();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
    }
    fn input(&mut self, kind: InputKind, title: &str, text: String) {
        let cursor = text.len();
        self.dialog = Some(Dialog::Input {
            kind,
            title: title.into(),
            text,
            cursor,
        });
    }
    fn command(&mut self, ch: char) -> Result<()> {
        if self.job.is_some() {
            return Ok(());
        }
        match ch {
            'o' => self.activate(false, true)?,
            'c' | 'x' => {
                let p = self.chosen();
                self.status = format!(
                    "{}개 {} 대기 · 대상 폴더에서 v",
                    p.len(),
                    if ch == 'c' { "복사" } else { "이동" }
                );
                self.clipboard = Some((p, ch == 'c'));
            }
            'v' => {
                if let Some((sources, copy)) = self.clipboard.clone() {
                    self.begin_transfer(sources, self.directory(), copy, false)?;
                }
            }
            'm' => self.input(
                InputKind::Move,
                "이동할 폴더 경로",
                self.directory().to_string_lossy().into(),
            ),
            'r' => {
                if let Some(p) = self.chosen().first().filter(|_| self.chosen().len() == 1) {
                    self.input(
                        InputKind::Rename(p.clone()),
                        "이름 변경",
                        p.file_name().unwrap_or_default().to_string_lossy().into(),
                    );
                }
            }
            'd' => {
                let p = self.chosen();
                if !p.is_empty() {
                    self.dialog = Some(Dialog::Confirm(p));
                }
            }
            'i' => self.input(
                InputKind::Import,
                "외부 파일 경로 (복사 · 공백 경로는 따옴표)",
                String::new(),
            ),
            'l' => self.input(
                InputKind::Location,
                "탐색할 폴더",
                self.directory().to_string_lossy().into(),
            ),
            'h' => {
                self.anchor = None;
                self.hidden = !self.hidden;
                self.refresh()?;
            }
            's' => {
                self.anchor = None;
                self.sort = match self.sort {
                    Sort::Folders => Sort::Name,
                    Sort::Name => Sort::Recent,
                    Sort::Recent => Sort::Folders,
                };
                self.refresh()?;
            }
            'f' => self.refresh()?,
            'p' => {
                if let Some(p) = self.manager.root.parent() {
                    self.change_root(p.to_path_buf())?;
                }
            }
            'q' => self.quit = true,
            _ => {}
        }
        Ok(())
    }
    fn begin_transfer(
        &mut self,
        sources: Vec<PathBuf>,
        dest: PathBuf,
        copy: bool,
        external: bool,
    ) -> Result<()> {
        let dest = self.manager.validate(&dest, true)?;
        let mut sources = sources;
        sources.sort();
        sources.dedup();
        let all = sources.clone();
        sources.retain(|p| {
            !all.iter()
                .any(|q| p != q && p.starts_with(q) && !q.is_symlink())
        });
        self.transfer_next(Transfer {
            sources,
            dest,
            copy,
            external,
            index: 0,
            completed: 0,
            name: None,
            policy: Policy::Error,
        })
    }
    fn transfer_next(&mut self, mut plan: Transfer) -> Result<()> {
        // Keep every selected source protected, including sources in a later batch.
        let mut operations = Vec::new();
        for source in &plan.sources {
            let target = plan.dest.join(source.file_name().unwrap_or_default());
            for original in &plan.sources {
                if original != source
                    && (original == &target
                        || original.starts_with(&target)
                        || original.canonicalize().ok().is_some_and(|p| {
                            target.canonicalize().ok().is_some_and(|t| p.starts_with(t))
                        }))
                {
                    bail!("선택한 원본을 덮어쓸 수 없습니다.");
                }
            }
        }
        while plan.index < plan.sources.len() {
            if plan.policy == Policy::Skip {
                plan.index += 1;
                plan.name = None;
                plan.policy = Policy::Error;
                continue;
            }
            let source = &plan.sources[plan.index];
            let target = if let Some(name) = &plan.name {
                plan.dest.join(name)
            } else {
                plan.dest.join(source.file_name().unwrap_or_default())
            };
            for original in &plan.sources {
                if original != source && (original == &target || original.starts_with(&target)) {
                    bail!("선택한 원본을 덮어쓸 수 없습니다.");
                }
            }
            if target == *source {
                plan.index += 1;
                plan.name = None;
                continue;
            }
            if std::fs::symlink_metadata(&target).is_ok() && matches!(plan.policy, Policy::Error) {
                // Run the already planned prefix first; conflict dialog continues the remaining batch.
                if !operations.is_empty() {
                    break;
                }
                self.dialog = Some(Dialog::Conflict(plan));
                return Ok(());
            }
            operations.push((source.clone(), plan.name.take(), plan.policy));
            plan.policy = Policy::Error;
            plan.index += 1;
        }
        if operations.is_empty() {
            self.status = format!("{}개 완료", plan.completed);
            return Ok(());
        }
        let root = self.manager.root.clone();
        let dest = plan.dest.clone();
        let copy = plan.copy;
        let external = plan.external;
        // Continuation is stored as a pending dialog-free plan and polled after the worker.
        let count = operations.len();
        let (tx, rx) = mpsc::channel();
        self.job = Some(rx);
        self.status = "파일 처리 중…".into();
        std::thread::spawn(move || {
            let result = (|| {
                let manager = Manager::new(root)?;
                let mut done = 0;
                for (source, name, policy) in operations {
                    match manager.transfer(&source, &dest, copy, policy, name.as_deref(), external)
                    {
                        Ok(Some(_)) => done += 1,
                        Ok(None) => {}
                        Err(e) => bail!("{done}개 완료 후 중단: {e}"),
                    }
                }
                Ok(format!(
                    "{done}개 {} 완료",
                    if copy { "복사" } else { "이동" }
                ))
            })();
            let _ = tx.send(result);
        });
        // A batch with a later conflict is resumed after completion by pending_transfer.
        plan.completed += count;
        if plan.index < plan.sources.len() {
            self.pending_transfer = Some(plan);
        }
        if !copy && !external {
            self.clipboard = None;
        }
        Ok(())
    }
    fn submit(&mut self, kind: InputKind, text: String) -> Result<()> {
        match kind {
            InputKind::Location => self.change_root(expand(&text, &self.directory()))?,
            InputKind::Move => self.begin_transfer(
                self.chosen(),
                expand(&text, &self.directory()),
                false,
                false,
            )?,
            InputKind::Import => {
                self.dialog = Some(Dialog::ImportConfirm(paths(&text, &self.directory())?));
            }
            InputKind::Rename(source) => {
                let target = source.parent().unwrap().join(&text);
                if std::fs::symlink_metadata(&target).is_ok() && target != source {
                    self.dialog = Some(Dialog::RenameConflict(source, text));
                } else {
                    self.rename(source, text, false);
                }
            }
            InputKind::ConflictName(mut plan) => {
                plan.name = Some(text);
                plan.policy = Policy::Error;
                self.transfer_next(plan)?;
            }
        }
        Ok(())
    }
    fn rename(&mut self, source: PathBuf, name: String, overwrite: bool) {
        let root = self.manager.root.clone();
        self.spawn(move || {
            Manager::new(root)?.rename(&source, &name, overwrite)?;
            Ok("이름 변경 완료".into())
        });
    }
    fn menu_action(&mut self, index: usize) -> Result<()> {
        if let Some(ch) = "ocxvmrdi".chars().nth(index) {
            self.command(ch)?;
        }
        Ok(())
    }
    fn key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind == KeyEventKind::Release {
            return Ok(());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
            if self.job.is_some() {
                self.status = "작업 완료 후 종료하세요.".into();
            } else {
                self.quit = true;
            }
            return Ok(());
        }
        if let Some(dialog) = self.dialog.take() {
            match dialog {
                Dialog::Input {
                    kind,
                    title,
                    mut text,
                    mut cursor,
                } => {
                    match key.code {
                        KeyCode::Esc => return Ok(()),
                        KeyCode::Enter => {
                            return self.submit(kind, text);
                        }
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            text.insert(cursor, c);
                            cursor += c.len_utf8();
                        }
                        KeyCode::Backspace => {
                            if cursor > 0 {
                                let prev = text[..cursor].char_indices().last().unwrap().0;
                                text.replace_range(prev..cursor, "");
                                cursor = prev;
                            }
                        }
                        KeyCode::Delete => {
                            if cursor < text.len() {
                                let n = text[cursor..].chars().next().unwrap().len_utf8();
                                text.replace_range(cursor..cursor + n, "");
                            }
                        }
                        KeyCode::Left => {
                            if cursor > 0 {
                                cursor = text[..cursor].char_indices().last().unwrap().0;
                            }
                        }
                        KeyCode::Right => {
                            if cursor < text.len() {
                                cursor += text[cursor..].chars().next().unwrap().len_utf8();
                            }
                        }
                        KeyCode::Home => cursor = 0,
                        KeyCode::End => cursor = text.len(),
                        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            text.clear();
                            cursor = 0;
                        }
                        _ => {}
                    }
                    self.dialog = Some(Dialog::Input {
                        kind,
                        title,
                        text,
                        cursor,
                    });
                }
                Dialog::Confirm(p) => match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        let root = self.manager.root.clone();
                        self.spawn(move || {
                            let n = Manager::new(root)?.trash(&p)?;
                            Ok(format!("{n}개 휴지통으로 이동"))
                        });
                    }
                    KeyCode::Esc | KeyCode::Char('n') => {}
                    _ => self.dialog = Some(Dialog::Confirm(p)),
                },
                Dialog::ImportConfirm(p) => match key.code {
                    KeyCode::Char('y') | KeyCode::Enter => {
                        self.begin_transfer(p, self.directory(), true, true)?
                    }
                    KeyCode::Esc | KeyCode::Char('n') => {}
                    _ => self.dialog = Some(Dialog::ImportConfirm(p)),
                },
                Dialog::Conflict(mut p) => match key.code {
                    KeyCode::Char('o') => {
                        p.policy = Policy::Overwrite;
                        self.transfer_next(p)?;
                    }
                    KeyCode::Char('r') => {
                        let name = p.sources[p.index]
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into();
                        self.input(InputKind::ConflictName(p), "다른 이름", name);
                    }
                    KeyCode::Char('s') => {
                        p.policy = Policy::Skip;
                        self.transfer_next(p)?;
                    }
                    KeyCode::Esc | KeyCode::Char('c') => {
                        self.status = format!("취소 · 앞선 작업 {}개 완료", p.completed)
                    }
                    _ => self.dialog = Some(Dialog::Conflict(p)),
                },
                Dialog::RenameConflict(p, name) => match key.code {
                    KeyCode::Char('o') => self.rename(p, name, true),
                    KeyCode::Char('r') => self.input(InputKind::Rename(p), "다른 이름", name),
                    KeyCode::Esc | KeyCode::Char('c') => {}
                    _ => self.dialog = Some(Dialog::RenameConflict(p, name)),
                },
                Dialog::Menu(mut i) => {
                    match key.code {
                        KeyCode::Esc => return Ok(()),
                        KeyCode::Down => i = (i + 1) % 8,
                        KeyCode::Up => i = (i + 7) % 8,
                        KeyCode::Enter => return self.menu_action(i),
                        KeyCode::Char(c) if "ocxvmrdi".contains(c) => {
                            return self.command(c);
                        }
                        _ => {}
                    }
                    self.dialog = Some(Dialog::Menu(i));
                }
            }
            return Ok(());
        }
        if self.job.is_some() {
            return Ok(());
        }
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                let col = &mut self.columns[self.active];
                if !col.entries.is_empty() {
                    let old = col.state.selected().unwrap_or(0);
                    let n = if key.code == KeyCode::Up {
                        old.saturating_sub(1)
                    } else {
                        (old + 1).min(col.entries.len() - 1)
                    };
                    col.state.select(Some(n));
                    if key.modifiers.contains(KeyModifiers::SHIFT) {
                        let a = self
                            .anchor
                            .filter(|(c, _)| *c == self.active)
                            .map(|(_, i)| i)
                            .unwrap_or(old);
                        self.anchor = Some((self.active, a));
                        self.selected = col.entries[a.min(n)..=a.max(n)]
                            .iter()
                            .map(|e| e.path.clone())
                            .collect();
                    } else {
                        self.selected.clear();
                    }
                }
            }
            KeyCode::Right => {
                if self.current().is_some_and(|e| e.is_dir) {
                    self.activate(false, false)?;
                    if self.active + 1 < self.columns.len() {
                        self.active += 1;
                        self.selected.clear();
                    }
                }
            }
            KeyCode::Left => {
                self.active = self.active.saturating_sub(1);
                self.selected.clear();
            }
            KeyCode::Enter => self.activate(false, true)?,
            KeyCode::Backspace => self.command('p')?,
            KeyCode::F(2) => self.command('r')?,
            KeyCode::F(5) => self.refresh()?,
            KeyCode::F(10) => self.dialog = Some(Dialog::Menu(0)),
            KeyCode::Delete => self.command('d')?,
            KeyCode::Char(' ') => {
                if let Some(e) = self.current() {
                    if !self.selected.remove(&e.path) {
                        self.selected.insert(e.path);
                    }
                }
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match c {
                        'l' => self.command('l')?,
                        'd' => self.command('d')?,
                        'a' => {
                            self.selected = self.columns[self.active]
                                .entries
                                .iter()
                                .map(|e| e.path.clone())
                                .collect();
                        }
                        _ => {}
                    }
                } else {
                    self.command(c)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn hit(&self, x: u16, y: u16) -> Option<(usize, usize)> {
        self.areas.iter().find_map(|(c, r)| {
            if x > r.x
                && x < r.right().saturating_sub(1)
                && y > r.y
                && y < r.bottom().saturating_sub(1)
            {
                let n = self.columns[*c].state.offset() + (y - r.y - 1) as usize;
                if n < self.columns[*c].entries.len() {
                    Some((*c, n))
                } else {
                    None
                }
            } else {
                None
            }
        })
    }
    fn mouse(&mut self, m: MouseEvent) -> Result<()> {
        if self.dialog.is_some() {
            if m.kind == MouseEventKind::Down(MouseButton::Left) {
                if let Some((_, code)) = self
                    .dialog_buttons
                    .iter()
                    .find(|(r, _)| r.contains((m.column, m.row).into()))
                {
                    return self.key(KeyEvent::new(*code, KeyModifiers::NONE));
                }
            }
            if m.kind == MouseEventKind::Down(MouseButton::Left)
                && self.dialog_area.contains((m.column, m.row).into())
            {
                let row = m.row.saturating_sub(self.dialog_area.y + 1) as usize;
                let code = match self.dialog.as_ref().unwrap() {
                    Dialog::Menu(_) if row < 8 => {
                        self.dialog = None;
                        return self.menu_action(row);
                    }
                    _ => None,
                };
                if let Some(code) = code {
                    return self.key(KeyEvent::new(code, KeyModifiers::NONE));
                }
            }
            return Ok(());
        }
        if self.job.is_some() {
            return Ok(());
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.open_button.contains((m.column, m.row).into()) {
                    self.command('o')?;
                    return Ok(());
                }
                if let Some((_, ch)) = self
                    .toolbar
                    .iter()
                    .find(|(r, _)| r.contains((m.column, m.row).into()))
                {
                    let ch = *ch;
                    self.command(ch)?;
                    return Ok(());
                }
                if let Some((c, n)) = self.hit(m.column, m.row) {
                    self.active = c;
                    self.columns[c].state.select(Some(n));
                    let e = self.columns[c].entries[n].clone();
                    let multi = m
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER);
                    if m.modifiers.contains(KeyModifiers::SHIFT) {
                        let a = self
                            .anchor
                            .filter(|(ac, _)| *ac == c)
                            .map(|(_, i)| i)
                            .unwrap_or(n);
                        self.selected = self.columns[c].entries[a.min(n)..=a.max(n)]
                            .iter()
                            .map(|e| e.path.clone())
                            .collect();
                    } else if multi {
                        if !self.selected.remove(&e.path) {
                            self.selected.insert(e.path.clone());
                        }
                    } else if !self.selected.contains(&e.path) {
                        self.selected = BTreeSet::from([e.path.clone()]);
                    }
                    self.anchor = Some((c, n));
                    self.drag = Some((c, e.path, multi));
                    self.dragged = false;
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.dragged = true,
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some((c, path, copy)) = self.drag.take() {
                    if self.dragged {
                        if let Some((dc, dn)) = self.hit(m.column, m.row) {
                            let e = &self.columns[dc].entries[dn];
                            let dest = if e.is_dir {
                                e.path.clone()
                            } else {
                                self.columns[dc].path.clone()
                            };
                            let sources = self.chosen();
                            self.begin_transfer(sources, dest, copy, false)?;
                        } else if let Some((dc, _)) = self
                            .areas
                            .iter()
                            .find(|(_, r)| r.contains((m.column, m.row).into()))
                        {
                            self.begin_transfer(
                                self.chosen(),
                                self.columns[*dc].path.clone(),
                                copy,
                                false,
                            )?;
                        }
                    } else if self.hit(m.column, m.row).is_some_and(|(hc, hn)| {
                        hc == c && self.columns[hc].entries[hn].path == path
                    }) && !m.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::SUPER | KeyModifiers::SHIFT,
                    ) {
                        let double = self.last_click.as_ref().is_some_and(|(p, t)| {
                            p == &path && t.elapsed() < Duration::from_millis(550)
                        });
                        self.active = c;
                        let folder = self.current().is_some_and(|e| e.is_dir);
                        self.activate(folder, double && !folder)?;
                        self.last_click = if double || folder {
                            None
                        } else {
                            Some((path, Instant::now()))
                        };
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some((c, n)) = self.hit(m.column, m.row) {
                    self.active = c;
                    self.columns[c].state.select(Some(n));
                    let p = self.columns[c].entries[n].path.clone();
                    if !self.selected.contains(&p) {
                        self.selected = BTreeSet::from([p]);
                    }
                    self.dialog = Some(Dialog::Menu(0));
                }
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                if let Some((c, _)) = self
                    .areas
                    .iter()
                    .find(|(_, r)| r.contains((m.column, m.row).into()))
                {
                    self.active = *c;
                    let col = &mut self.columns[*c];
                    let n = col.state.selected().unwrap_or(0);
                    if !col.entries.is_empty() {
                        col.state
                            .select(Some(if m.kind == MouseEventKind::ScrollDown {
                                (n + 3).min(col.entries.len() - 1)
                            } else {
                                n.saturating_sub(3)
                            }));
                        self.selected.clear();
                    }
                }
            }
            MouseEventKind::ScrollLeft => {
                self.active = self.active.saturating_sub(1);
            }
            MouseEventKind::ScrollRight => {
                self.active = (self.active + 1).min(self.columns.len() - 1);
            }
            _ => {}
        }
        Ok(())
    }
    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        if area.width < 30 || area.height < 10 {
            f.render_widget(Paragraph::new("터미널을 넓혀주세요 (30×10 이상)"), area);
            return;
        }
        let header = Rect::new(area.x, area.y, area.width, 1);
        f.render_widget(
            Paragraph::new(
                Line::from(" FinderViewer  ·  Rust").style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ),
            header,
        );
        let root = Rect::new(area.x, area.y + 1, area.width, 1);
        f.render_widget(
            Paragraph::new(format!(" {}", clean(self.manager.root.display()))),
            root,
        );
        self.toolbar.clear();
        let mut x = area.x;
        for (label, ch, width) in [
            (" 상위 ", 'p', 8),
            (" 새로고침 ", 'f', 12),
            (
                if self.hidden {
                    " 숨김 ON "
                } else {
                    " 숨김 OFF "
                },
                'h',
                12,
            ),
            (
                match self.sort {
                    Sort::Folders => " 폴더 우선 ",
                    Sort::Name => " 이름순 ",
                    Sort::Recent => " 최근순 ",
                },
                's',
                14,
            ),
        ] {
            if x + width <= area.right() {
                let rect = Rect::new(x, area.y + 2, width, 1);
                f.render_widget(
                    Paragraph::new(label).style(Style::default().fg(Color::Black).bg(Color::Gray)),
                    rect,
                );
                self.toolbar.push((rect, ch));
                x += width + 1;
            }
        }
        let content = Rect::new(area.x, area.y + 3, area.width, area.height - 8);
        let visible = usize::from((content.width / 28).max(1));
        let reveal = if self.columns.get(self.active + 1).is_some() {
            self.active + 1
        } else {
            self.active
        };
        let start = reveal.saturating_sub(visible - 1);
        let count = (self.columns.len() - start).min(visible);
        let width = if visible == 1 {
            content.width
        } else {
            content.width / visible as u16
        };
        self.areas.clear();
        for n in 0..count {
            let c = start + n;
            let r = Rect::new(
                content.x + n as u16 * width,
                content.y,
                width,
                content.height,
            );
            self.areas.push((c, r));
            let col = &mut self.columns[c];
            let title = col
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into());
            let block = Block::default()
                .borders(Borders::ALL)
                .title(clean(title))
                .border_style(Style::default().fg(if c == self.active {
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
                        let marked = self.selected.contains(&e.path);
                        let icon = entry_icon(e);
                        let name_style = if marked {
                            Style::default().fg(Color::Cyan)
                        } else if e.is_dir {
                            Style::default()
                                .fg(Color::Yellow)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        };
                        let padding = " ".repeat(
                            2usize.saturating_sub(unicode_width::UnicodeWidthStr::width(icon)),
                        );
                        ListItem::new(Line::from(vec![
                            Span::raw(if marked { "✓ " } else { "  " }),
                            Span::raw(format!("{icon}{padding} ")),
                            Span::styled(clean(&e.name), name_style),
                        ]))
                    })
                    .collect();
                let list = List::new(items)
                    .block(block)
                    .highlight_style(if c == self.active {
                        Style::default()
                            .bg(Color::Blue)
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().bg(Color::DarkGray)
                    });
                f.render_stateful_widget(list, r, &mut col.state);
            }
        }
        if start > 0 {
            f.render_widget(
                Paragraph::new("◀ 이전 열: ←"),
                Rect::new(content.x, content.y, 16, 1),
            );
        }
        let y = area.bottom() - 5;
        let details = if let Some(e) = self.current() {
            let date: chrono::DateTime<chrono::Local> = e.modified.into();
            format!(
                " {} · {} · {} · {}개 선택\n {}",
                clean(&e.name),
                if e.is_dir {
                    "폴더".into()
                } else {
                    format!("{} bytes", e.size)
                },
                date.format("%Y-%m-%d %H:%M"),
                self.chosen().len(),
                clean(e.path.display())
            )
        } else {
            " 선택한 항목 없음".into()
        };
        f.render_widget(Paragraph::new(details), Rect::new(area.x, y, area.width, 2));
        self.open_button = Rect::new(area.x, y + 2, 18, 1);
        let enabled = self.current().is_some() && self.job.is_none();
        f.render_widget(
            Paragraph::new("  열기 [Enter/o]  ").style(
                Style::default()
                    .fg(if enabled { Color::Black } else { Color::Gray })
                    .bg(if enabled {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            self.open_button,
        );
        if area.width > 20 {
            f.render_widget(
                Paragraph::new(" F10 메뉴 · c/x/v · F2 이름 · Ctrl+D 삭제"),
                Rect::new(area.x + 19, y + 2, area.width - 19, 1),
            );
        }
        f.render_widget(
            Paragraph::new(format!(" {}", clean(&self.status)))
                .style(Style::default().fg(Color::Yellow)),
            Rect::new(area.x, y + 3, area.width, 1),
        );
        f.render_widget(
            Paragraph::new(" i 가져오기 · h 숨김 · s 정렬 · Ctrl+L 경로 · q 종료")
                .style(Style::default().fg(Color::DarkGray)),
            Rect::new(area.x, y + 4, area.width, 1),
        );
        self.dialog_buttons.clear();
        if let Some(dialog) = &self.dialog {
            let w = area.width.saturating_sub(4).min(84);
            let h = match dialog {
                Dialog::Menu(_) => 12,
                _ => 8,
            }
            .min(area.height - 2);
            let r = Rect::new(
                area.x + (area.width - w) / 2,
                area.y + (area.height - h) / 2,
                w,
                h,
            );
            self.dialog_area = r;
            f.render_widget(Clear, r);
            let (title,text)=match dialog{
 Dialog::Input{title,text,..}=>(title.clone(),format!("{}\n\nEnter 확인 · Esc 취소 · Ctrl+A 비우기",clean(text))),
 Dialog::Confirm(p)=>("휴지통으로 이동".into(),format!("{}개 항목을 휴지통으로 이동할까요?\n{}\n\ny / Enter 확인 · n / Esc 취소",p.len(),clean(p.iter().take(2).map(|p|p.file_name().unwrap_or_default().to_string_lossy()).collect::<Vec<_>>().join(", ")))),
 Dialog::ImportConfirm(p)=>("외부 파일 가져오기".into(),format!("{}개를 현재 열에 복사합니다. 원본 유지.\n{}\n\ny / Enter 확인 · n / Esc 취소",p.len(),clean(self.directory().display()))),
 Dialog::Conflict(p)=>("같은 이름이 있습니다".into(),format!("{}\n\no 덮어쓰기 (기존 항목은 휴지통)\nr 다른 이름 · s 건너뛰기 · Esc 취소",clean(p.name.clone().unwrap_or_else(||p.sources[p.index].file_name().unwrap_or_default().to_string_lossy().into())))),
 Dialog::RenameConflict(_,name)=>("같은 이름이 있습니다".into(),format!("{}\n\no 덮어쓰기 (기존 항목은 휴지통)\nr 다른 이름 · Esc 취소",clean(name))),
 Dialog::Menu(_)=>("작업 메뉴 (↑↓ Enter / 단축키)".into(),String::new())};
            let block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));
            if let Dialog::Menu(index) = dialog {
                let items = [
                    "열기 (o)",
                    "복사 (c)",
                    "잘라내기 (x)",
                    "붙여넣기 (v)",
                    "다른 폴더로 이동 (m)",
                    "이름 변경 (r / F2)",
                    "삭제 → 휴지통 (d)",
                    "외부 파일 가져오기 (i)",
                ];
                let mut state = ListState::default().with_selected(Some(*index));
                f.render_stateful_widget(
                    List::new(items)
                        .block(block)
                        .highlight_style(Style::default().bg(Color::Blue)),
                    r,
                    &mut state,
                );
            } else {
                f.render_widget(
                    Paragraph::new(text).block(block).wrap(Wrap { trim: false }),
                    r,
                );
                if let Dialog::Input { text, cursor, .. } = dialog {
                    let inner_width = r.width.saturating_sub(2).max(1);
                    let width =
                        unicode_width::UnicodeWidthStr::width(clean(&text[..*cursor]).as_str());
                    let x = r.x + 1 + (width % usize::from(inner_width)) as u16;
                    let y = r.y + 1 + (width / usize::from(inner_width)) as u16;
                    if y < r.bottom() - 1 {
                        f.set_cursor_position((x, y));
                    }
                }
            }
            let buttons: Vec<(&str, KeyCode)> = match dialog {
                Dialog::Confirm(_) | Dialog::ImportConfirm(_) => {
                    vec![("확인 [y]", KeyCode::Enter), ("취소 [Esc]", KeyCode::Esc)]
                }
                Dialog::Input { .. } => vec![
                    ("확인 [Enter]", KeyCode::Enter),
                    ("취소 [Esc]", KeyCode::Esc),
                ],
                Dialog::Conflict(_) => vec![
                    ("덮어쓰기 [o]", KeyCode::Char('o')),
                    ("다른 이름 [r]", KeyCode::Char('r')),
                    ("건너뛰기 [s]", KeyCode::Char('s')),
                    ("취소", KeyCode::Esc),
                ],
                Dialog::RenameConflict(_, _) => vec![
                    ("덮어쓰기 [o]", KeyCode::Char('o')),
                    ("다른 이름 [r]", KeyCode::Char('r')),
                    ("취소", KeyCode::Esc),
                ],
                Dialog::Menu(_) => vec![],
            };
            let mut bx = r.x + 1;
            for (label, code) in buttons {
                let width = unicode_width::UnicodeWidthStr::width(label) as u16 + 2;
                if bx + width >= r.right() {
                    break;
                }
                let rect = Rect::new(bx, r.bottom() - 2, width, 1);
                f.render_widget(
                    Paragraph::new(format!(" {label} "))
                        .style(Style::default().bg(Color::Blue).fg(Color::White)),
                    rect,
                );
                self.dialog_buttons.push((rect, code));
                bx += width + 1;
            }
        }
    }
}
#[cfg(unix)]
const MOUSE_CELL_MODE: &[u8] =
    b"\x1b[?1000l\x1b[?1003l\x1b[?1005l\x1b[?1015l\x1b[?1016l\x1b[?1002h\x1b[?1006h";
fn entry_icon(entry: &Entry) -> &'static str {
    if entry.is_symlink {
        return "🔗";
    }
    if entry.is_dir {
        return "📁";
    }
    match entry
        .path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "📕",
        "doc" | "docx" | "odt" => "📘",
        "xls" | "xlsx" | "csv" => "📗",
        "ppt" | "pptx" => "📙",
        "zip" | "gz" | "tar" | "7z" | "rar" => "📦",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" => "🎨",
        "mp3" | "wav" | "flac" | "m4a" | "ogg" => "🎵",
        "mp4" | "mov" | "mkv" | "webm" | "avi" => "🎬",
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "go" | "c" | "cpp" | "java" | "sh" => "🧩",
        _ => "📄",
    }
}
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
    }
}
pub fn run(root: PathBuf) -> Result<()> {
    let mut app = App::new(root)?;
    enable_raw_mode()?;
    let _guard = Guard;
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    // Crossterm also enables RXVT and all-motion modes. Use only cell-based
    // SGR button/drag reports: nested terminals can misencode mixed protocols.
    #[cfg(unix)]
    {
        use std::io::Write;
        io::stdout().write_all(MOUSE_CELL_MODE)?;
        io::stdout().flush()?;
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut dirty = true;
    while !app.quit {
        if let Some(rx) = &app.job {
            match rx.try_recv() {
                Ok(result) => {
                    dirty = true;
                    app.job = None;
                    match result {
                        Ok(status) => {
                            app.status = status;
                            if let Some(plan) = app.pending_transfer.take() {
                                if let Err(e) = app.transfer_next(plan) {
                                    app.status = format!("{e}");
                                }
                            }
                        }
                        Err(e) => {
                            app.pending_transfer = None;
                            app.status = format!("오류: {e}");
                        }
                    }
                    if let Err(e) = app.refresh() {
                        app.status = format!("오류: {e}");
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    dirty = true;
                    app.job = None;
                    app.pending_transfer = None;
                    app.status = "작업 스레드가 중단됐습니다.".into();
                }
                Err(_) => {}
            }
        }
        if app.job.is_none()
            && app.dialog.is_none()
            && app.last_refresh.elapsed() > Duration::from_secs(2)
        {
            dirty = true;
            if let Err(e) = app.refresh() {
                app.status = format!("새로고침: {e}");
                app.last_refresh = Instant::now();
            }
        }
        if dirty {
            terminal.draw(|f| app.draw(f))?;
            dirty = false;
        }
        if event::poll(Duration::from_millis(80))? {
            dirty = true;
            let result = match event::read()? {
                Event::Key(key) => app.key(key),
                Event::Mouse(m) => app.mouse(m),
                Event::Paste(text) => {
                    if let Some(Dialog::Input {
                        text: ref mut value,
                        cursor,
                        ..
                    }) = app.dialog.as_mut()
                    {
                        value.insert_str(*cursor, &text);
                        *cursor += text.len();
                    } else if app.dialog.is_none() && app.job.is_none() {
                        match paths(&text, &app.directory()) {
                            Ok(p) => app.dialog = Some(Dialog::ImportConfirm(p)),
                            Err(e) => app.status = format!("경로 오류: {e}"),
                        }
                    }
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(e) = result {
                app.status = format!("오류: {e}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn columns_toggle_and_parent_selection() {
        let d = tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b/c")).unwrap();
        std::fs::create_dir(d.path().join("z")).unwrap();
        let mut a = App::new(d.path().into()).unwrap();
        a.activate(true, false).unwrap();
        assert_eq!(a.columns.len(), 2);
        a.activate(true, false).unwrap();
        assert_eq!(a.columns.len(), 1);
        a.activate(false, false).unwrap();
        a.active = 1;
        a.activate(false, false).unwrap();
        assert_eq!(a.columns.len(), 3);
        a.active = 0;
        a.columns[0].state.select(Some(1));
        a.activate(true, false).unwrap();
        assert_eq!(a.columns.len(), 2);
        assert!(a.columns[1].path.ends_with("z"));
    }
    #[test]
    fn bottom_button_uses_same_open_action() {
        let d = tempdir().unwrap();
        std::fs::create_dir(d.path().join("folder")).unwrap();
        let mut a = App::new(d.path().into()).unwrap();
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| a.draw(f)).unwrap();
        let r = a.open_button;
        a.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: r.x + 1,
            row: r.y,
            modifiers: KeyModifiers::NONE,
        })
        .unwrap();
        assert_eq!(a.columns.len(), 2);
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(text.replace(' ', "").contains("열기"));
    }
    #[test]
    fn unicode_edit_and_paste_paths() {
        let d = tempdir().unwrap();
        let mut a = App::new(d.path().into()).unwrap();
        a.input(InputKind::Location, "경로", "한 글".into());
        a.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        a.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        if let Some(Dialog::Input { text, .. }) = &a.dialog {
            assert_eq!(text, "한글");
        } else {
            panic!();
        }
        assert_eq!(
            paths("'한 글.txt'", d.path()).unwrap()[0],
            d.path().join("한 글.txt")
        );
    }
    #[test]
    fn empty_folder_navigation_does_not_panic() {
        let d = tempdir().unwrap();
        let mut a = App::new(d.path().into()).unwrap();
        for code in [
            KeyCode::Up,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Right,
            KeyCode::Left,
        ] {
            a.key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
        }
        assert!(a.current().is_none());
    }
    #[test]
    fn menu_contains_delete_and_open() {
        let d = tempdir().unwrap();
        let mut a = App::new(d.path().into()).unwrap();
        a.dialog = Some(Dialog::Menu(6));
        let mut t = Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
        t.draw(|f| a.draw(f)).unwrap();
        let s = t
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(s.replace(' ', "").contains("삭제"));
        assert!(s.replace(' ', "").contains("열기"));
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod extra_tests;
