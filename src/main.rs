mod app;
mod fs;
mod ssh;
mod ssh_app;
use anyhow::{bail, Context, Result};
use std::{ffi::OsString, path::PathBuf};

#[derive(Debug, PartialEq)]
enum Launch {
    Local(Option<PathBuf>),
    Remote(String, PathBuf),
    Help,
    Version,
}
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Launch> {
    let mut args = args.into_iter();
    let result = match args.next() {
        Some(arg) if arg == "--help" || arg == "-h" => Launch::Help,
        Some(arg) if arg == "--version" || arg == "-V" => Launch::Version,
        Some(arg) if arg == "--ssh" => {
            let target = args
                .next()
                .context("사용법: finderviewer --ssh 사용자@서버 [원격 폴더]")?
                .into_string()
                .map_err(|_| anyhow::anyhow!("SSH 호스트는 UTF-8이어야 합니다."))?;
            ssh::validate_target(&target)?;
            Launch::Remote(
                target,
                args.next()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("~")),
            )
        }
        Some(arg) if arg == "--" => Launch::Local(args.next().map(PathBuf::from)),
        Some(arg) => Launch::Local(Some(PathBuf::from(arg))),
        None => Launch::Local(None),
    };
    if args.next().is_some() {
        bail!("폴더 경로는 한 개만 지정하세요. finderviewer --help 참고");
    }
    Ok(result)
}
fn main() -> Result<()> {
    match parse_args(std::env::args_os().skip(1))? {
        Launch::Help=>println!("FinderViewer {} (Rust)\n사용법: finderviewer [폴더]\n        finderviewer --ssh 사용자@서버 [원격 폴더]\n별칭: finderview\n로컬: Enter/o 또는 하단 열기로 기본 앱 실행\nSSH: 읽기 전용 탐색, 파일을 내려받아 로컬 기본 앱에서 열기\n서버에 Python 3, 로컬에 OpenSSH 필요. ~/.ssh/config 별칭 지원.\nF5 새로고침 · Ctrl+L 경로 · q 종료 · Ctrl+Q 항상 종료", env!("CARGO_PKG_VERSION")),
        Launch::Version=>println!("finderviewer {} (Rust)",env!("CARGO_PKG_VERSION")),
        Launch::Local(path)=>app::run(path.unwrap_or(std::env::current_dir()?))?,
        Launch::Remote(target,path)=>ssh_app::run(target,path)?,
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Launch> {
        parse_args(args.iter().map(OsString::from))
    }
    #[test]
    fn ssh_cli_preserves_remote_path_and_defaults_to_remote_home() {
        assert_eq!(
            parse(&["--ssh", "user@host"]).unwrap(),
            Launch::Remote("user@host".into(), "~".into())
        );
        assert_eq!(
            parse(&["--ssh", "server-alias", "/한글 공백/'file"]).unwrap(),
            Launch::Remote("server-alias".into(), "/한글 공백/'file".into())
        );
    }
    #[test]
    fn ssh_cli_rejects_missing_target_options_and_extra_paths() {
        for args in [
            &["--ssh"][..],
            &["--ssh", "-oProxyCommand=bad"],
            &["--ssh", "user@host", "/a", "/b"],
        ] {
            assert!(parse(args).is_err());
        }
    }
    #[test]
    fn local_cli_still_supports_current_and_explicit_paths() {
        assert_eq!(parse(&[]).unwrap(), Launch::Local(None));
        assert_eq!(parse(&["."]).unwrap(), Launch::Local(Some(".".into())));
        assert_eq!(
            parse(&["--", "--ssh"]).unwrap(),
            Launch::Local(Some("--ssh".into()))
        );
    }
}
