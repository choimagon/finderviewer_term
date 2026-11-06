mod app;
mod fs;
use anyhow::{bail, Result};
use std::path::PathBuf;
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let root = match args.next() {
        Some(arg) if arg == "--help" || arg == "-h" => {
            println!("FinderViewer {} (Rust)\n사용법: finderviewer [폴더]\n별칭: finderview\nEnter/o 또는 하단 열기: 기본 앱 열기 · F10: 메뉴 · q: 종료", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(arg) if arg == "--version" || arg == "-V" => {
            println!("finderviewer {} (Rust)", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(arg) => PathBuf::from(arg),
        None => std::env::current_dir()?,
    };
    if args.next().is_some() {
        bail!("폴더 경로 한 개만 지정하세요.");
    }
    app::run(root)
}
