# Rust Clean Browser

[![Release](https://img.shields.io/github/v/release/Hannoah/rust-clean-browser)](https://github.com/Hannoah/rust-clean-browser/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

순수 Rust(Blitz `Stylo + Taffy + Parley + Vello CPU`) 기반의 초경량·메모리 최적화 웹 브라우저입니다.  
비표준 레거시 웹과 무거운 스크립트/트래커 실행을 배제하고, **표준 HTML/CSS의 빠른 렌더링**과 **극한의 메모리 절약**에 집중합니다.

## 설치

[**최신 릴리스 다운로드**](https://github.com/Hannoah/rust-clean-browser/releases/latest)

- **Windows 10/11**: `RustCleanBrowser-Setup-<버전>-x64.exe` 실행 (관리자 권한 불필요, 시작 메뉴·바탕화면 아이콘·제거 프로그램 등록, 기본 브라우저 후보 등록 선택 가능). 설치 없이 쓰려면 `portable.zip`.
- **macOS 11+ (Apple Silicon)**: `.dmg`를 열어 앱을 Applications로 드래그.
- 코드 서명이 없는 빌드라 첫 실행 시 SmartScreen에서는 **추가 정보 → 실행**, macOS에서는 **우클릭 → 열기**를 선택하세요.

## 주요 기능

- **최신 Chrome / Edge 스타일 프레임리스 UI**
  - 탭 스트립이 타이틀바를 겸함: 빈 영역 드래그로 창 이동, 더블클릭으로 최대화/복원, 가장자리 드래그로 크기 조절
  - 자체 최소화/최대화/닫기 버튼, Windows 11 둥근 모서리·그림자, Segoe Fluent 아이콘(이모지 폰트 미사용 → 메모리 절감)
  - 시스템 라이트/다크 모드 자동 대응(크롬 UI·새 탭·북마크 관리자·리더 모드)
  - 알약형 주소창(🔒 표시, ☆ 북마크), 새 탭 페이지 검색창, 줌 배율 표시(클릭 시 100%)
  - 주소창에 URL 입력 시 자동 `https://` 보정, 검색어 입력 시 Google 검색(JS 모드)으로 연결
- **로그인 유지**
  - Pure Rust 모드 쿠키를 `cookies.json`에 저장해 재시작 후에도 로그인 유지
  - JS 모드(WebView2)는 탭 전환으로 WebView를 해제하기 전에 세션 쿠키를 30일 쿠키로 보존, 비밀번호 저장·자동 완성 활성화
- **사이트 아이콘(파비콘)**: 탭·북마크 바·팝업·새 탭 페이지에 각 서비스가 `<link rel=icon>`으로 선언한 고유 아이콘 표시(SVG 포함, JS 모드는 로그인된 WebView에서 읽음, 64px 디스크 캐시). Confluence/Jira처럼 한 도메인의 여러 서비스도 구분
- **호환성**
  - `<meta charset>` 감지 및 EUC-KR 자동 디코딩(한글 레거시 사이트 깨짐 방지)
  - 폼 POST 전송 지원, 키보드 스크롤(↑↓ / PageUp·Down / Space / Home·End)
  - JS 모드(WebView2)에서 Windows 계정 SSO 사용: 회사 Entra ID 조건부 액세스(규정 준수 기기 확인)가 Edge처럼 통과 — Microsoft 365/Copilot 로그인 차단 해결
  - Microsoft/Google 로그인 도메인은 처음부터 JS 모드로 열림
- **공격적 메모리 최적화**
  - **비활성 탭 즉시 언로드**: 다른 탭으로 전환하면 떠나는 탭의 DOM·스타일 트리·레이아웃·이미지 버퍼를 즉시 `Drop`하고 URL과 스크롤 위치만 유지합니다.
  - **Windows Working Set 즉시 회수**: 무거운 탭을 닫거나 전환할 때 `SetProcessWorkingSetSize`로 해제된 힙 페이지를 OS에 즉시 반환합니다.
  - **컴팩트 HTML LRU 캐시**: 파싱 전 `<script>`·광고 `<iframe>`·대용량 주석을 제거한 경량 HTML만 최대 8개(2MB 상한)로 캐시하며, 활성 탭과 캐시가 같은 버퍼(`Arc<str>`)를 공유해 중복 복사가 없습니다.
  - **이벤트 기반 렌더링**: 마우스 이동마다 전체 화면을 다시 그리지 않고 hover 상태가 실제로 바뀔 때만 다시 그립니다(마우스 이동 시 CPU 약 1/3).
  - **크롬 UI 증분 갱신**: 상태가 바뀌지 않으면 UI 문서를 다시 파싱하지 않으며, 백그라운드 이벤트가 와도 주소창 입력 중인 텍스트를 보존합니다.
  - **Ctrl+클릭 = 절전 상태 백그라운드 탭**: 방문하기 전까지 DOM 메모리 0.
  - **공유 폰트 컨텍스트**: 전체 탭과 Chrome UI가 단일 `FontContext`를 공유합니다.
  - **실시간 메모리 모니터링**: 상단 우측 배지에서 현재 프로세스 Working Set(MB), 슬립 탭 수, 차단된 트래커 수를 실시간으로 확인할 수 있습니다.
- **Zero-JS & 트래커 차단 (`Clean` 모드) + 온디맨드 `JS` 하이브리드**
  - 일반/표준 페이지는 **순수 Rust Blitz 엔진**으로 `<script>` 및 주요 광고·트래커 도메인을 차단한 채 초저메모리로 렌더링합니다.
  - `www.naver.com`처럼 본문이 비어 있는 CSR JavaScript SPA 페이지가 감지되면 해당 활성 탭만 자동으로 **`JS (WebView2)` 호환 모드**로 전환(상단 `JS` 버튼으로 언제든 수동 토글 가능)하고, 다른 탭으로 전환하면 가장 최근 JS 탭 1개의 WebView만 숨겨 두고(돌아올 때 새로고침 없이 즉시 표시) 그 이전 것은 즉시 해제해 메모리를 회수합니다.
- **리더 모드 (`Reader` / `Alt+R`)**
  - 복잡한 페이지에서 본문(`<article>`/`<main>`)만 정제해 깔끔한 읽기 화면으로 전환합니다.
- **Chrome 북마크 자동 가져오기**
  - `%LOCALAPPDATA%\Google\Chrome\User Data\<Profile>\Bookmarks`를 읽어 북마크 바 및 `rust://bookmarks` 전체 목록에 표시합니다.
  - 상단 `☆/★` 버튼으로 추가한 개인 북마크는 `%LOCALAPPDATA%\rust-browser\bookmarks.json`에 저장됩니다.

## 단축키

| 단축키 | 동작 |
| :--- | :--- |
| `Ctrl+T` | 새 탭 열기 + 주소창 포커스 |
| `Ctrl+W` | 현재 탭 닫기 |
| `Ctrl+Shift+T` | 닫은 탭 다시 열기 |
| `Ctrl+Shift+Tab` / `Ctrl+1~9` | 이전 탭 / N번째(9=마지막) 탭 |
| `Ctrl+D` | 현재 페이지 북마크 토글 |
| `Ctrl+클릭` | 링크를 백그라운드 탭으로 열기 |
| `Esc` | 로딩 중지 / 주소창 포커스 해제 |
| `Alt+Home` | 새 탭 페이지 |
| `Ctrl+L` / `Alt+D` | 주소창 포커스 + 전체 선택 |
| `Ctrl+Tab` | 다음 탭으로 전환 |
| `Ctrl+R` / `F5` | 현재 탭 새로고침 |
| `Alt+←` / `Alt+→` | 뒤로 / 앞으로 |
| `Alt+R` | 리더 모드 토글 |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | 확대 / 축소 / 100% 초기화 |

## 빌드 및 실행

```powershell
cargo test
cargo build --release
.\target\release\rust-browser.exe
```

### macOS

```bash
bash scripts/macos-install.sh   # 빌드 → ~/Applications/Rust Clean Browser.app 설치 + 바탕화면 바로가기
```

- 필요: Rust(`rustup`), Xcode Command Line Tools(`xcode-select --install`)
- macOS에서는 네이티브 신호등 버튼(투명 타이틀바)을 쓰고, 단축키는 `Ctrl` 대신 `Cmd`(`Cmd+T/W/L/R/D/1~9`, `Cmd+[`/`]` 뒤로/앞으로, `Cmd+Q` 종료)입니다. JS 모드는 WKWebView를 사용합니다.
- 북마크는 `~/Library/Application Support/Google/Chrome`, 앱 데이터는 `~/Library/Application Support/rust-browser`를 사용합니다.

### 아이콘

`python assets/make_icon.py`로 재생성합니다. `icon.ico`는 빌드 시 `windres`로 exe에 임베드되고, `icon.icns`는 macOS 앱 번들, `icon_64.rgba`는 창/작업표시줄 아이콘입니다.

## 라이선스

[MIT](LICENSE). `vendor/`의 서드파티 크레이트는 각자의 라이선스(MIT OR Apache-2.0)를 따릅니다 — [vendor/README.md](vendor/README.md) 참고.
