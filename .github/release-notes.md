초경량 순수 Rust 웹 브라우저 — 적은 메모리, 빠른 로딩, Chrome 북마크 연동.
*Ultra-lightweight pure-Rust web browser — low memory, instant page loads, Chrome bookmark import.*

## 이번 버전 (v0.4.0)

- **모던 UI/UX 대폭 강화 (Fluent 2 + Material 3 + Arc/Zen 스타일)**
  - **크로스플랫폼 벡터 SVG 아이콘**: Windows·macOS 전반에서 동일한 선명한 1.5px 벡터 아이콘 적용 및 Pretendard/Segoe UI Variable 기반 한국어 CJK 타이포그래피·`tabular-nums` 정비
  - **레이어드 탭 스트립 & 상단 툴바**: 절전 탭 시각 구분, `JS`/`R` 모드 배지, 호버 시 닫기 버튼 강조, 2px 상단 로딩 프로그레스 바, 자동 소멸 상태 알림 토스트(`status_note`), 접을 수 있는 북마크 바(`Ctrl+Shift+B`)
  - **커맨드 팔레트 (`Ctrl+K` / `Ctrl+Shift+P`)**: 열린 탭 전환, 브라우저 명령 실행, 테마 순환(시스템/라이트/다크), 북마크·방문 기록 통합 검색
  - **페이지 내 찾기 (`Ctrl+F`)**: Blitz 문서 실시간 매치 카운트(`1 / N`) 및 스크롤 이동·텍스트 하이라이트, JS 모드 `window.find` 연동
  - **향상된 주소창 자동완성 & 링크 미리보기**: 열린 탭 후보·일치어 강조·카테고리 배지, 페이지 내 링크 호버 시 좌하단 URL 프리뷰 필 표시
  - **리더 모드 (`Alt+R`) 강화**: 라이트/세피아/다크 테마, 글자 크기(`14~24px`), 고딕/명조 서체, 본문 폭(`56/66/78ch`) 조절, 예상 읽기 시간 표시
  - **Bento 새 탭 대시보드(`rust://home`) & 방문 기록 타임라인(`rust://history`, `Ctrl+H`)**: 메모리 비교 게이지, 최근 방문 기록 카드, 북마크 개별 삭제 지원, 오류 페이지에서 `JS 모드로 열기` 원클릭 복구
  - **JS 모드 백그라운드 절전 & 한글 입력기(IME) 개선**: 최대 5개 WebView2 백그라운드 일시 중지(`TrySuspend`)·즉시 복원, 한/영 IME 포커스 전환 안정화, 가운데 클릭 새 탭/탭 닫기 지원

### v0.3.0

- **주소창 자동완성**: 입력하는 동안 방문 기록·북마크에서 후보를 드롭다운으로 표시
  - ↑↓ 선택, Enter 이동, Esc 닫기, 클릭(Ctrl+클릭은 새 탭)
  - 주소 앞부분이 일치하면 첫 후보를 미리 선택(인라인 자동완성)
  - 방문 기록 저장(`history.json`, 최대 5000개) — 기록은 이번 버전부터 쌓입니다

### v0.2.1

- 마우스를 창 아래쪽(짧은 페이지의 빈 영역)으로 내리면 포인터가 사라지던 문제 수정
- 내부 코드 정리(불필요한 필드·중복 코드 제거, 동작 변화 없음)
- 릴리스 빌드 파이프라인을 최신 GitHub Actions(Node 24)로 갱신

### v0.2.0

- **새 앱 아이콘**: 모던한 스퀴클 디자인
- **탭 전환 시 새로고침 감소**: 가장 최근 JS 탭 1개는 WebView를 숨겨 두어 돌아오면 즉시 표시
- **주소창 기본 검색 Google**로 변경
- **사이트 아이콘 정확도 개선**: 각 서비스가 선언한 고유 아이콘 사용(SVG 지원, Confluence/Jira 등 구분, 로그인된 JS 페이지 아이콘 반영)
- **주소창 입력 시 경고음 제거**(JS 탭 이후 키 입력마다 '띵' 소리 나던 문제)
- 높은 화면 배율(125%/150%)에서 JS 배지·메모리 버튼 테두리가 끊겨 보이던 문제 수정

## 다운로드 / Download

| 플랫폼 | 파일 | 설명 |
| --- | --- | --- |
| Windows 10/11 (x64) | `RustCleanBrowser-Setup-*-x64.exe` | **설치 파일 (권장)** — 시작 메뉴·바탕화면 아이콘, 제거 프로그램 등록, 관리자 권한 불필요 |
| Windows 10/11 (x64) | `RustCleanBrowser-*-windows-x64-portable.zip` | 설치 없이 압축 풀고 실행 |
| macOS 11+ (Apple Silicon) | `RustCleanBrowser-*-macos-arm64.dmg` | DMG를 열고 앱을 Applications로 드래그 |

## 주요 기능

- 탭 전환 시 비활성 탭 메모리 즉시 해제, 새 탭 ~60 MB
- 방문했던 페이지는 디스크 캐시에서 즉시 표시 후 백그라운드로 최신화 (stale-while-revalidate)
- 로그인 유지: 쿠키 디스크 저장, JS 모드 비밀번호 저장, 회사 계정(Entra ID) SSO
- 사이트 파비콘, 라이트/다크 모드, 프레임리스 최신 UI
- 트래커·광고 차단, 리더 모드, JS가 필요한 사이트는 자동으로 JS 모드(WebView2/WKWebView)

## 처음 실행 시 경고 안내

이 빌드는 코드 서명이 되어 있지 않습니다.
- **Windows SmartScreen**: "Windows의 PC 보호" 창에서 **추가 정보 → 실행**을 누르세요.
- **macOS Gatekeeper**: 앱을 **우클릭 → 열기**를 선택하거나, 터미널에서
  `xattr -dr com.apple.quarantine "/Applications/Rust Clean Browser.app"` 를 실행하세요.

License: MIT
