use super::*;
use tempfile::{tempdir, TempDir};

fn fixture() -> (TempDir, App) {
    let dir = tempdir().unwrap();
    let app = App::new(dir.path().to_path_buf()).unwrap();
    (dir, app)
}
fn render(app: &mut App) {
    let mut terminal = Terminal::new(ratatui::backend::TestBackend::new(120, 32)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
}
fn position(app: &App, col: usize, name: &str) -> (u16, u16) {
    let index = app.columns[col]
        .entries
        .iter()
        .position(|e| e.name == name)
        .unwrap();
    let rect = app.areas.iter().find(|(c, _)| *c == col).unwrap().1;
    (
        rect.x + 2,
        rect.y + 1 + (index - app.columns[col].state.offset()) as u16,
    )
}
fn mouse_at(app: &mut App, kind: MouseEventKind, pos: (u16, u16), modifiers: KeyModifiers) {
    app.mouse(MouseEvent {
        kind,
        column: pos.0,
        row: pos.1,
        modifiers,
    })
    .unwrap();
}
fn click(app: &mut App, col: usize, name: &str) {
    render(app);
    let pos = position(app, col, name);
    mouse_at(
        app,
        MouseEventKind::Down(MouseButton::Left),
        pos,
        KeyModifiers::NONE,
    );
    mouse_at(
        app,
        MouseEventKind::Up(MouseButton::Left),
        pos,
        KeyModifiers::NONE,
    );
}
fn press(app: &mut App, code: KeyCode) {
    app.key(KeyEvent::new(code, KeyModifiers::NONE)).unwrap();
}
fn finish(app: &mut App) {
    while let Some(rx) = app.job.take() {
        let result = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("worker timeout")
            .unwrap();
        app.status = result;
        app.refresh().unwrap();
        if let Some(plan) = app.pending_transfer.take() {
            app.transfer_next(plan).unwrap();
        }
    }
}
#[test]
fn mouse_drag_moves_file_into_folder() {
    let (dir, mut app) = fixture();
    std::fs::create_dir(dir.path().join("목적 폴더")).unwrap();
    std::fs::write(dir.path().join("한글 파일.txt"), b"unchanged").unwrap();
    app.refresh().unwrap();
    render(&mut app);
    let source = position(&app, 0, "한글 파일.txt");
    let dest = position(&app, 0, "목적 폴더");
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        source,
        KeyModifiers::NONE,
    );
    mouse_at(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        dest,
        KeyModifiers::NONE,
    );
    mouse_at(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        dest,
        KeyModifiers::NONE,
    );
    finish(&mut app);
    assert!(!dir.path().join("한글 파일.txt").exists());
    assert_eq!(
        std::fs::read(dir.path().join("목적 폴더/한글 파일.txt")).unwrap(),
        b"unchanged"
    );
}
#[test]
fn modifier_drag_copies_without_removing_original() {
    let (dir, mut app) = fixture();
    std::fs::create_dir(dir.path().join("dest")).unwrap();
    std::fs::write(dir.path().join("file.pdf"), b"binary").unwrap();
    app.refresh().unwrap();
    render(&mut app);
    let source = position(&app, 0, "file.pdf");
    let dest = position(&app, 0, "dest");
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        source,
        KeyModifiers::CONTROL,
    );
    mouse_at(
        &mut app,
        MouseEventKind::Drag(MouseButton::Left),
        dest,
        KeyModifiers::CONTROL,
    );
    mouse_at(
        &mut app,
        MouseEventKind::Up(MouseButton::Left),
        dest,
        KeyModifiers::CONTROL,
    );
    finish(&mut app);
    assert!(dir.path().join("file.pdf").exists());
    assert_eq!(
        std::fs::read(dir.path().join("dest/file.pdf")).unwrap(),
        b"binary"
    );
}
#[test]
fn clipboard_paste_uses_open_folder_even_when_parent_column_has_focus() {
    let (dir, mut app) = fixture();
    std::fs::create_dir(dir.path().join("dest")).unwrap();
    std::fs::write(dir.path().join("file.docx"), b"document").unwrap();
    app.refresh().unwrap();
    click(&mut app, 0, "file.docx");
    app.command('x').unwrap();
    click(&mut app, 0, "dest");
    assert_eq!(app.active, 0);
    assert!(app.directory().ends_with("dest"));
    app.command('v').unwrap();
    finish(&mut app);
    assert!(!dir.path().join("file.docx").exists());
    assert_eq!(
        std::fs::read(dir.path().join("dest/file.docx")).unwrap(),
        b"document"
    );
    assert!(app.clipboard.is_none());
}
#[test]
fn same_name_conflict_can_rename_without_modifying_existing_file() {
    let (dir, mut app) = fixture();
    std::fs::create_dir(dir.path().join("dest")).unwrap();
    std::fs::write(dir.path().join("file"), b"new").unwrap();
    std::fs::write(dir.path().join("dest/file"), b"old").unwrap();
    app.begin_transfer(
        vec![dir.path().join("file")],
        dir.path().join("dest"),
        true,
        false,
    )
    .unwrap();
    assert!(matches!(app.dialog, Some(Dialog::Conflict(_))));
    assert!(app.job.is_none());
    press(&mut app, KeyCode::Char('r'));
    let kind = match app.dialog.take().unwrap() {
        Dialog::Input { kind, .. } => kind,
        _ => panic!("rename input missing"),
    };
    app.submit(kind, "다른 이름".into()).unwrap();
    finish(&mut app);
    assert_eq!(std::fs::read(dir.path().join("dest/file")).unwrap(), b"old");
    assert_eq!(
        std::fs::read(dir.path().join("dest/다른 이름")).unwrap(),
        b"new"
    );
    assert!(dir.path().join("file").exists());
}
#[test]
fn conflict_skip_and_cancel_leave_source_and_destination_untouched() {
    for choice in [KeyCode::Char('s'), KeyCode::Esc] {
        let (dir, mut app) = fixture();
        std::fs::create_dir(dir.path().join("dest")).unwrap();
        std::fs::write(dir.path().join("file"), b"new").unwrap();
        std::fs::write(dir.path().join("dest/file"), b"old").unwrap();
        app.begin_transfer(
            vec![dir.path().join("file")],
            dir.path().join("dest"),
            false,
            false,
        )
        .unwrap();
        press(&mut app, choice);
        assert!(app.dialog.is_none());
        finish(&mut app);
        assert_eq!(std::fs::read(dir.path().join("file")).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.path().join("dest/file")).unwrap(), b"old");
    }
}
#[test]
fn delete_keyboard_and_context_menu_require_confirmation_and_support_cancel() {
    let (dir, mut app) = fixture();
    let file = dir.path().join("keep.txt");
    std::fs::write(&file, b"keep").unwrap();
    app.refresh().unwrap();
    press(&mut app, KeyCode::Delete);
    assert!(matches!(app.dialog, Some(Dialog::Confirm(_))));
    assert!(app.job.is_none());
    assert!(file.exists());
    press(&mut app, KeyCode::Esc);
    assert!(app.dialog.is_none());
    assert!(file.exists());
    app.dialog = Some(Dialog::Menu(6));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.dialog, Some(Dialog::Confirm(_))));
    assert!(app.job.is_none());
    press(&mut app, KeyCode::Char('n'));
    assert!(file.exists());
    assert!(app.dialog.is_none());
}
#[test]
fn three_depth_navigation_recovers_when_open_ancestor_is_deleted() {
    let (dir, mut app) = fixture();
    std::fs::create_dir_all(dir.path().join("one/two/three")).unwrap();
    app.refresh().unwrap();
    click(&mut app, 0, "one");
    click(&mut app, 1, "two");
    click(&mut app, 2, "three");
    assert_eq!(app.columns.len(), 4);
    std::fs::remove_dir_all(dir.path().join("one/two")).unwrap();
    app.active = 3;
    app.refresh().unwrap();
    assert_eq!(app.columns.len(), 2);
    assert_eq!(app.active, 1);
    assert!(app.selected.is_empty());
    assert!(app.columns[1].entries.is_empty());
}
#[test]
fn selection_anchor_remains_valid_after_directory_shrinks() {
    let (dir, mut app) = fixture();
    for n in 0..8 {
        std::fs::write(dir.path().join(format!("file{n}")), b"x").unwrap();
    }
    app.refresh().unwrap();
    click(&mut app, 0, "file7");
    for n in 1..8 {
        std::fs::remove_file(dir.path().join(format!("file{n}"))).unwrap();
    }
    app.refresh().unwrap();
    app.key(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(app.chosen().len(), 1);
}

fn fake_open_file(path: &Path) -> Result<()> {
    std::fs::write(
        path.parent().unwrap().join(".opened-marker"),
        path.to_string_lossy().as_bytes(),
    )?;
    Ok(())
}
fn assert_opened_exact_file(dir: &Path, file: &Path) {
    assert_eq!(
        std::fs::read_to_string(dir.join(".opened-marker")).unwrap(),
        file.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(std::fs::read(file).unwrap(), b"original binary bytes\0\xff");
}
#[test]
fn file_single_click_selects_and_bottom_button_opens_exact_unicode_path() {
    let (dir, mut app) = fixture();
    let file = dir.path().join("한글 공백 파일.docx");
    std::fs::write(&file, b"original binary bytes\0\xff").unwrap();
    app.opener = fake_open_file;
    app.refresh().unwrap();
    click(&mut app, 0, "한글 공백 파일.docx");
    assert!(app.job.is_none());
    assert!(!dir.path().join(".opened-marker").exists());
    assert_eq!(app.chosen(), vec![file.canonicalize().unwrap()]);
    render(&mut app);
    let pos = (app.open_button.x + 2, app.open_button.y);
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        pos,
        KeyModifiers::NONE,
    );
    finish(&mut app);
    assert_opened_exact_file(dir.path(), &file);
}
#[test]
fn enter_and_o_open_selected_file_without_modifying_contents() {
    for key in [KeyCode::Enter, KeyCode::Char('o')] {
        let (dir, mut app) = fixture();
        let file = dir.path().join("한글 공백 파일.pdf");
        std::fs::write(&file, b"original binary bytes\0\xff").unwrap();
        app.opener = fake_open_file;
        app.refresh().unwrap();
        click(&mut app, 0, "한글 공백 파일.pdf");
        press(&mut app, key);
        finish(&mut app);
        assert_opened_exact_file(dir.path(), &file);
    }
}
#[test]
fn sequential_doubleclick_opens_file_without_relying_on_terminal_doubleclick_event() {
    let (dir, mut app) = fixture();
    let file = dir.path().join("파일 이름.xlsx");
    std::fs::write(&file, b"original binary bytes\0\xff").unwrap();
    app.opener = fake_open_file;
    app.refresh().unwrap();
    click(&mut app, 0, "파일 이름.xlsx");
    assert!(app.job.is_none());
    assert!(!dir.path().join(".opened-marker").exists());
    click(&mut app, 0, "파일 이름.xlsx");
    finish(&mut app);
    assert_opened_exact_file(dir.path(), &file);
}
#[test]
fn rightclick_selects_exact_row_and_menu_delete_requests_confirmation() {
    let (dir, mut app) = fixture();
    std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
    let selected = dir.path().join("우클릭 파일.txt");
    std::fs::write(&selected, b"keep").unwrap();
    app.refresh().unwrap();
    render(&mut app);
    let pos = position(&app, 0, "우클릭 파일.txt");
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Right),
        pos,
        KeyModifiers::NONE,
    );
    assert!(matches!(app.dialog, Some(Dialog::Menu(0))));
    assert_eq!(app.chosen(), vec![selected.canonicalize().unwrap()]);
    press(&mut app, KeyCode::Char('d'));
    match &app.dialog {
        Some(Dialog::Confirm(paths)) => assert_eq!(paths, &vec![selected.canonicalize().unwrap()]),
        _ => panic!("delete confirm missing"),
    }
    assert!(app.job.is_none());
    press(&mut app, KeyCode::Esc);
    assert_eq!(std::fs::read(selected).unwrap(), b"keep");
}
#[test]
fn file_uri_parser_decodes_unicode_spaces_and_rejects_remote_hosts() {
    let (_, app) = fixture();
    assert_eq!(
        paths("file:///tmp/%ED%95%9C%20%EA%B8%80.txt", &app.manager.root).unwrap(),
        vec![PathBuf::from("/tmp/한 글.txt")]
    );
    assert_eq!(
        paths("file://localhost/tmp/a%20b.pdf", &app.manager.root).unwrap(),
        vec![PathBuf::from("/tmp/a b.pdf")]
    );
    assert!(paths("file://remote.example/tmp/a", &app.manager.root).is_err());
}
#[test]
fn external_uri_import_requires_confirmation_and_preserves_original() {
    let (dir, mut app) = fixture();
    let external = tempdir().unwrap();
    let source = external.path().join("외부 공백.pptx");
    std::fs::write(&source, b"external bytes").unwrap();
    let uri = format!("file://{}", source.to_string_lossy().replace(' ', "%20"));
    app.submit(InputKind::Import, uri).unwrap();
    assert!(matches!(app.dialog, Some(Dialog::ImportConfirm(_))));
    assert!(app.job.is_none());
    assert!(!dir.path().join("외부 공백.pptx").exists());
    press(&mut app, KeyCode::Char('y'));
    finish(&mut app);
    assert_eq!(
        std::fs::read(dir.path().join("외부 공백.pptx")).unwrap(),
        b"external bytes"
    );
    assert_eq!(std::fs::read(source).unwrap(), b"external bytes");
}

#[test]
fn repeated_folder_click_toggles_even_within_doubleclick_interval() {
    let (dir, mut app) = fixture();
    std::fs::create_dir(dir.path().join("folder")).unwrap();
    app.refresh().unwrap();
    click(&mut app, 0, "folder");
    assert_eq!(app.columns.len(), 2);
    click(&mut app, 0, "folder");
    assert_eq!(app.columns.len(), 1);
}

#[test]
fn modal_cancel_button_accepts_mouse_click_and_preserves_file() {
    let (dir, mut app) = fixture();
    let file = dir.path().join("keep.txt");
    std::fs::write(&file, b"keep").unwrap();
    app.refresh().unwrap();
    app.command('d').unwrap();
    render(&mut app);
    let rect = app
        .dialog_buttons
        .iter()
        .find(|(_, code)| *code == KeyCode::Esc)
        .unwrap()
        .0;
    mouse_at(
        &mut app,
        MouseEventKind::Down(MouseButton::Left),
        (rect.x + 1, rect.y),
        KeyModifiers::NONE,
    );
    assert!(app.dialog.is_none());
    assert!(file.exists());
    assert!(app.job.is_none());
}
