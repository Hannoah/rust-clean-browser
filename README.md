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

- **모던 Fluent 2 / Chrome Refresh 스타일 프레임리스 UI**
  - 탭 스트립이 타이틀바를 겸함: 빈 영역 드래그로 창 이동, 더블클릭으로 최대화/복원, 가장자리 드래그로 크기 조절, 탭 순서 드래그 변경, 탭을 탭 영역 밖으로 드래그해 새 창으로 분리, 절전 탭·`JS`/`R` 모드 배지 표시
  - 자체 최소화/최대화/닫기 SVG 버튼, Windows 11 둥근 모서리·그림자, 크로스플랫폼 벡터 SVG 아이콘(이모지 폰트 미사용 → 메모리 절감)
  - 시스템 / 라이트 / 다크 테마 수동 전환(`prefs.json` 저장) 및 자동 대응(크롬 UI·새 탭·북마크·방문 기록·리더 모드)
  - 알약형 주소창(🔒/⚠/🧭 표시, 인라인 리더 모드·☆ 북마크·줌 배율), 2px 로딩 프로그레스 바, 자동 소멸 상태 토스트, 좌하단 링크 호버 프리뷰, 북마크 바 토글(`Ctrl+Shift+B`)
  - **AI 어시스턴트 사이드바 (`Ctrl+Shift+I`)**: 현재 페이지를 읽고 요약·번역·질의응답. 로그인된 로컬 CLI(Gemini·Claude·Codex·Muse) 또는 API(OpenAI 호환/Ollama·Anthropic·Gemini) 사용, 대화/기록/설정 탭, API는 컨텍스트 사용량 표시와 대화 압축, 답변 타이핑 효과, 드래그로 일부만 선택해 `Ctrl+C` 복사, `Shift+Enter` 줄바꿈, 파일 첨부(📎 버튼 또는 사이드바로 끌어다 놓기: 이미지·PDF·텍스트·코드 파일, 캡처 이미지는 `Ctrl+V`로 붙여넣기), 글자 크기 조절, 대화 기록을 이 PC에 저장(Windows는 DPAPI 암호화)·불러오기·삭제. Gemini·Claude 에이전트는 설정에서 effort(추론 강도: 자동/낮음/중간/높음) 조절 가능. 사용 가능한 제공자(설치된 CLI·키가 저장된 API)만 목록에 표시, API는 키로 실제 사용 가능한 모델 목록을 불러오고, 목록에 없는 모델 ID 추가와 1-토큰 연결 테스트 지원. 작업 중 상태와 사용한 도구를 카드로 표시. `제어` 모드에서는 AI가 JS 모드 페이지를 클릭·입력·이동하고 페이지 내 찾기·다른 탭 읽기·PDF 쪽 읽기/검색·폼 일괄 입력·북마크/방문 기록 검색·인용·화면 캡처 도구를 쓰며, 열려 있지 않거나 요청에 없는 사이트로 이동할 때는 허용 여부를 묻습니다
  - **자동 업데이트**: 하루 1회 GitHub 릴리스 확인, 새 버전이 있으면 상단에 `업데이트` 버튼 표시 → 클릭 한 번으로 설치 파일 다운로드·실행
  - **첫 실행 환영**: 설치 후 첫 실행에 Chrome·Edge·Whale 북마크 가져오기를 물어보고, 동의하면 프로필별로 가져옴
  - **커맨드 팔레트 (`Ctrl+K`)**: 열린 탭, 브라우저 명령, 북마크, 방문 기록을 한 곳에서 즉시 검색·실행
  - **출처 복사**: 팔레트에서 `출처` 검색 → IEEE / APA 7 / 발표자료 / MLA 9 형식 인용문을 클립보드로 복사(저자·사이트명·발행일은 페이지 메타에서 자동 추출)
  - **PDF 보기·필기**: `.pdf` 주소·로컬 파일·확장자 없는 PDF 응답도 항상 내장 pdf.js 편집기로 자동 열림(검색·확대/축소·페이지 이동·목차·인쇄 지원) — 형광펜·손글씨(그리기)·텍스트 메모·이미지 삽입, 저장 시 주석이 포함된 PDF를 다운로드 폴더에 저장. 로컬 파일과 http(s) PDF 모두 지원, PDF 내 JavaScript는 비활성, 설치 시 `.pdf` 연결 프로그램 후보 등록
  - **소셜 로그인 호환**: Google/Microsoft/Apple/Kakao/Naver 로그인 팝업(`window.open`)을 실제 팝업 창으로 열어 `window.opener` 연동 유지(빈 화면 방지), Google·Apple·Kakao·Naver 로그인 SDK가 들어간 페이지는 자동으로 JS 모드로 열림
  - **한영 번역 (`Alt+T`)**: 한글 페이지는 영어로, 그 외는 한국어로 자동 방향 감지 번역(팔레트 `번역`). 본문 텍스트가 Google의 비공식 번역 엔드포인트로 전송되며(언제든 차단·변경될 수 있어 실패할 수 있음), 다시 누르면 원문 복원(Clean 모드 전용)
  - **파일/URL 실행 시 새 탭**: 브라우저가 이미 열려 있으면 `.html` 등을 더블클릭해도 새 창 대신 기존 창의 새 탭으로 열림
  - **페이지 내 찾기 (`Ctrl+F`)**: 실시간 매치 카운트(`1 / N`) 및 스크롤/하이라이트(JS 모드 포함)
  - **Bento 새 탭 대시보드 & 방문 기록(`rust://history`, `Ctrl+H`)**: 빠른 실행 타일, 메모리 절감 게이지, 최근 방문 기록, 시간순 기록 관리
  - **주소창 자동완성**: 입력하는 동안 열린 탭·방문 기록·북마크에서 일치어 강조와 함께 후보를 드롭다운으로 표시(↑↓ 선택, Enter 이동, Esc 닫기)
  - 주소창에 URL 입력 시 자동 `https://` 보정, 검색어 입력 시 Google 검색(JS 모드)으로 연결
- **로그인 유지**
  - Pure Rust 모드 쿠키를 `cookies.json`에 저장해 재시작 후에도 로그인 유지(Windows는 DPAPI로 사용자 계정에 묶어 암호화, 그 외 플랫폼은 소유자 전용 파일 권한 0600)
  - JS 모드(WebView2)는 탭 전환으로 WebView를 해제하기 전에 세션 쿠키를 30일 쿠키로 보존하며, 비밀번호 자동 저장과 일반 자동 완성은 보안상 비활성화되어 있습니다
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
  - `www.naver.com`처럼 본문이 비어 있는 CSR JavaScript SPA 페이지가 감지되면 해당 활성 탭만 자동으로 **`JS (WebView2)` 호환 모드**로 전환(상단 `JS` 버튼으로 언제든 수동 토글 가능)하고, 다른 탭으로 전환하면 최근 JS 탭 최대 5개의 WebView를 백그라운드 절전(`TrySuspend` + 저메모리 모드) 상태로 숨겨 두어 새로고침·세션 손실 없이 즉시 복원하며, 5개를 초과한 가장 오래된 탭은 즉시 해제해 메모리를 회수합니다. 카메라·마이크를 쓰거나 소리를 재생 중인 탭(화상회의·음악, 탭에 `LIVE` 표시)은 절전·해제하지 않아 탭을 옮겨도 끊기지 않습니다.
- **리더 모드 (`Reader` / `Alt+R`)**
  - 복잡한 페이지에서 본문(`<article>`/`<main>`)만 정제하고, 라이트/세피아/다크 테마·글자 크기·고딕/명조 서체·본문 폭·예상 읽기 시간을 제공합니다.
- **Chrome 북마크 자동 가져오기**
  - `%LOCALAPPDATA%\Google\Chrome\User Data\<Profile>\Bookmarks`를 읽어 북마크 바 및 `rust://bookmarks` 전체 목록에 표시합니다.
  - 상단 `☆/★` 버튼으로 추가한 개인 북마크는 `%LOCALAPPDATA%\rust-browser\bookmarks.json`에 저장됩니다.

## 단축키

| 단축키 | 동작 |
| :--- | :--- |
| `Ctrl+T` | 새 탭 열기 + 주소창 포커스 |
| `Ctrl+N` | 새 창 열기 |
| `Ctrl+K` / `Ctrl+Shift+P` | 커맨드 팔레트 열기 |
| `Ctrl+F` | 페이지 내 찾기 |
| `Ctrl+H` | 방문 기록 (`rust://history`) |
| `Ctrl+J` | 다운로드 패널 (실행 파일·스크립트는 다운로드 차단) |
| `Ctrl+Shift+B` | 북마크 바 표시/숨김 |
| `Ctrl+Shift+I` | AI 어시스턴트 사이드바 열기/닫기 |
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
| `Alt+T` | 한영 번역 토글 |
| `Ctrl++` / `Ctrl+-` / `Ctrl+0` | 확대 / 축소 / 100% 초기화 |
| `Ctrl+스크롤` | 마우스 휠로 확대 / 축소 |

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
