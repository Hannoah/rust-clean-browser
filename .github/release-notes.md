초경량 순수 Rust 웹 브라우저 — 적은 메모리, 빠른 로딩, Chrome 북마크 연동.
*Ultra-lightweight pure-Rust web browser — low memory, instant page loads, Chrome bookmark import.*

## 이번 버전 (v0.2.0)

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
