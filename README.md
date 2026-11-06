# FinderViewer Terminal

**터미널에서 Finder처럼 폴더를 열 단위로 탐색하는 파일 탐색기입니다.**

0.2.0부터 Rust로 만들었습니다. Python이나 가상환경 없이 실행 파일 하나로 사용할 수 있습니다. 폴더를 클릭하면 오른쪽 열이 펼쳐지고 파일 복사·이동·이름 변경·휴지통 삭제를 지원합니다. VS Code 확장과 별도로 실행합니다.

기본 명령은 **`finderviewer`**이며 **`finderview`는 같은 프로그램을 여는 별칭**입니다.

## 바로 설치 — Apple Silicon Mac

[릴리스](https://github.com/choimagon/finderviewer_term/releases/tag/v0.2.1)에서 **`finderviewer-v0.2.1-macos-arm64.tar.gz`**를 다운로드하세요. Apple Silicon Mac에서 빌드한 네이티브 실행 파일입니다. **Rust와 Python을 설치할 필요가 없습니다.** Intel Mac·Linux·Windows용 실행 파일은 제공하지 않으며 해당 환경은 검증하지 않았습니다.

다운로드한 폴더에서:

```sh
tar xzf finderviewer-v0.2.1-macos-arm64.tar.gz
cd finderviewer-v0.2.1-macos-arm64
sh scripts/install.sh --binary ./finderviewer
```

새 터미널에서 `finderviewer .`을 실행하세요. 셸을 지정하려면 `--shell zsh` 또는 `--shell bash`를 함께 쓸 수 있습니다. 명령만 설치하려면 `--shell none`을 지정합니다. 바이너리는 `~/.local/bin`에 복사되므로 압축을 푼 원본 폴더는 보관하지 않아도 됩니다.

macOS가 다운로드한 실행 파일을 차단하면 시스템 설정의 **개인정보 보호 및 보안**에서 해당 파일의 실행 허용 여부를 확인하세요. 함께 제공하는 `SHA256SUMS`로 다운로드 파일의 체크섬도 확인할 수 있습니다.

## 소스 빌드 설치 — macOS / Linux

소스 빌드에는 Git과 Rust 툴체인이 필요합니다. Rust가 없다면 [rustup 공식 안내](https://rustup.rs)에 따라 설치한 뒤 새 터미널을 여세요.

```sh
git clone https://github.com/choimagon/finderviewer_term.git
cd finderviewer_term
sh scripts/install.sh
```

설치 스크립트가 `cargo build --release --locked`로 빌드한 다음 실행 파일을 `~/.local/bin/finderviewer`에 복사합니다. 저장소를 옮기거나 지워도 설치한 바이너리는 실행됩니다. zsh/Bash에서는 PATH와 폴더 Tab 자동완성도 등록합니다. **새 터미널에서** 실행하세요.

```sh
finderviewer .
```

macOS/Linux용 자동 설치 스크립트입니다. Windows는 자동 설치·PowerShell 자동완성을 지원하지 않습니다. 각 운영체제에서의 검증 범위는 릴리스 안내를 확인하세요. 파일 열기는 운영체제의 기본 연결 앱을 사용하므로 해당 앱이 필요합니다.

### 설치 옵션

```sh
sh scripts/install.sh --shell zsh
sh scripts/install.sh --shell bash
sh scripts/install.sh --shell none
```

기본값 `auto`는 `$SHELL`을 기준으로 셸을 선택합니다. `none`은 셸 설정을 변경하지 않습니다. 이때 `~/.local/bin`을 PATH에 추가하거나 다음처럼 실행합니다.

```sh
~/.local/bin/finderviewer .
```

기존 Python FinderViewer의 알려진 설치 링크는 Rust 바이너리로 교체하며 백업 링크를 남깁니다. 다른 프로그램의 `finderviewer` 명령은 덮어쓰지 않고 중단합니다. `finderview` 별칭만 충돌하면 기존 명령을 보존하고 기본 명령을 설치합니다. 셸 설정은 변경 전에 백업하고 FinderViewer 관리 블록만 갱신합니다.

## 실행과 경로 자동완성

```sh
finderviewer .
finderviewer ~/Documents
finderviewer "/경로/한글 공백 폴더"
finderviewer
finderview .
```

인자를 생략하거나 `.`을 지정하면 현재 디렉터리에서 시작합니다. `finderviewer ~/Doc`까지 입력한 뒤 **Tab**을 누르면 `cd`처럼 폴더 경로를 완성합니다.

```sh
finderviewer --help
finderviewer --version
```

## 사용법

폴더 📁, 일반 파일 📄, PDF 📕, 워드 📘, 엑셀 📗, 발표 자료 📙, 압축파일 📦처럼 아이콘으로 구분합니다.

파일을 선택한 뒤 **하단의 열기 버튼**을 클릭하세요. 더블클릭이 터미널에서 잘 전달되지 않아도 버튼이나 `Enter`·`o`로 열 수 있습니다. 폴더는 안으로 탐색하고 파일은 OS 기본 앱으로 엽니다.

| 조작 | 동작 |
|---|---|
| 폴더 클릭 | 오른쪽 열에 하위 목록 표시, 같은 폴더 다시 클릭하면 접기 |
| 파일 클릭 | 이름·크기·수정 시간·경로 확인 |
| 하단 **열기** / Enter / `o` | 선택한 폴더 탐색 또는 파일을 기본 앱으로 열기 |
| 파일 더블클릭 | OS 기본 앱으로 열기 |
| ↑ / ↓ | 항목 선택 |
| → / ← | 폴더 탐색 / 이전 열로 이동 |
| Ctrl/Meta 클릭 / Shift 클릭 | 다중 선택 / 범위 선택 |
| Space / Ctrl+A | 항목 선택 토글 / 현재 열 전체 선택 |
| `c` / `x` | 복사 대기 / 잘라내기 대기 |
| `v` | 현재 열의 폴더에 붙여넣기 |
| 마우스로 폴더에 드래그 | 이동, Ctrl/Meta와 함께 드래그하면 복사 |
| `m` | 이동할 폴더 경로 입력 |
| F2 | 파일·폴더 이름 변경 |
| Delete / Ctrl+D | 확인 후 휴지통으로 이동 |
| 우클릭 / F10 | 파일 작업 메뉴 |
| `i` | 외부 파일·폴더 경로를 입력해 복사 |
| F5 / `h` | 새로고침 / 점(.) 파일 표시 토글 |
| `s` | 폴더 우선 → 이름순 → 최근 수정순 정렬 |
| Ctrl+L | 탐색할 시작 폴더 변경 |
| Backspace | 시작 폴더의 상위 폴더로 이동 |
| `q` / Ctrl+Q | 종료 |

마우스 이벤트를 전달하는 터미널이 필요합니다. 터미널이 우클릭을 자체 메뉴로 처리하면 **F10**을 사용하세요.

### 파일을 다른 폴더로 옮기기

1. 파일을 선택하고 `x`를 누릅니다.
2. 대상 폴더를 클릭해 들어갑니다.
3. `v`를 누릅니다. 복사하려면 처음에 `c`를 사용합니다.

우클릭/F10 메뉴나 `m`으로 대상 폴더 경로를 입력할 수도 있습니다.

### 외부 파일 가져오기

터미널에 파일을 드래그하면 보통 파일 객체 대신 경로가 입력됩니다. 대상 폴더를 연 뒤 `i`로 가져오기 입력 창을 열고 외부 파일을 그 입력 칸에 드래그하거나 경로를 붙여넣으세요. 확인 후 복사하며 원본은 유지합니다. 경로는 셸 명령으로 실행하지 않습니다.

### 이름 충돌과 삭제

같은 이름이 있으면 덮어쓰기·다른 이름·건너뛰기·취소 중에서 선택합니다. 덮어쓸 항목과 삭제 대상은 OS 휴지통으로 보냅니다. 휴지통을 사용할 수 없으면 중단하며 영구 삭제로 전환하지 않습니다.

현재 탐색 루트 밖의 항목을 변경하거나 시작 폴더 자체를 삭제·이동하지 않습니다. 다른 위치에서 작업하려면 Ctrl+L로 시작 폴더를 바꾸세요. 외부 파일 가져오기는 복사만 수행합니다. 심볼릭 링크를 통해 루트 밖을 탐색하는 작업은 제한합니다. 파일 작업 도중 오류가 나면 일부 작업이 완료됐을 수 있으므로 화면에 표시한 결과를 확인하세요.

## 업데이트

소스 빌드로 설치했다면 설치에 사용한 저장소에서 다음을 실행하고 프로그램을 다시 여세요. 바이너리로 설치했다면 새 릴리스 압축 파일을 받아 `--binary`로 다시 설치합니다.

```sh
git pull --ff-only
sh scripts/install.sh
```

프로젝트는 [GitHub 저장소](https://github.com/choimagon/finderviewer_term)와 [릴리스](https://github.com/choimagon/finderviewer_term/releases)로 배포합니다. 이전 v0.1.0 릴리스의 `.whl`·Python 소스 압축 파일은 Python 버전입니다. Rust 버전은 Apple Silicon용 바이너리를 다운로드하거나 위의 소스 빌드 절차를 사용하세요. PyPI의 동명 패키지는 이 프로젝트의 설치 경로가 아닙니다.

## 개발

```sh
cargo build --locked
cargo test --locked
cargo run -- .
cargo build --release --locked
./target/release/finderviewer .
```

Rust와 Ratatui/Crossterm을 사용합니다. 파일 작업은 Rust 표준 라이브러리와 휴지통 라이브러리로 처리합니다. 프로그램에 인터넷 전송이나 텔레메트리 기능은 없습니다. 빌드 시에는 Cargo가 의존 패키지를 다운로드할 수 있습니다.

기존 Python 소스와 테스트는 `legacy/python/`에 보관합니다. 새 Rust 실행 파일의 동작 검증과는 별도입니다.

Rust 자동 테스트 **30개**와 macOS 실제 터미널에서 하단 열기 버튼을 통한 기본 앱 실행, 드래그 이동, F2 한글 이름 변경, 휴지통 삭제를 확인했습니다. Linux/Windows의 실제 터미널·기본 앱·휴지통 동작은 해당 환경에서 추가 검증이 필요합니다.

문제나 제안은 [Issues](https://github.com/choimagon/finderviewer_term/issues)에 남겨주세요.

## 라이선스

MIT. [LICENSE](LICENSE)를 참고하세요.
