# 시스템에 맡긴 일(연결 프로그램, 관리자 권한, 터미널, 속성 창, Windows 메뉴, 공유)과 클립보드.

## 시스템이 하지 않은 일 (words::shell_failed). $reason: 오류 하나(error-*).

shell-open-with-unsupported = 이 시스템에서는 연결 프로그램 고르기를 지원하지 않습니다
shell-run-as-admin-unsupported = 이 시스템에서는 관리자 권한으로 실행할 수 없습니다
shell-terminal-unsupported = 터미널 프로그램을 찾지 못했습니다
shell-properties-unsupported = 이 시스템에서는 OS 속성 창을 지원하지 않습니다
shell-system-menu-unsupported = 이 시스템에는 Windows 메뉴가 없습니다
shell-share-unsupported = 이 시스템에서는 공유를 지원하지 않습니다
# macOS: Finder를 제어할 권한.
shell-finder-not-allowed = Finder를 제어할 권한이 없어 정보 창을 열지 못했습니다. 시스템 설정 ▸ 개인정보 보호 및 보안 ▸ 자동화에서 허용해 주세요
shell-open-with-failed = 연결 프로그램으로 열지 못했습니다: { $reason }
shell-run-as-admin-failed = 관리자 권한으로 실행하지 못했습니다: { $reason }
shell-terminal-failed = 터미널을 열지 못했습니다: { $reason }
shell-properties-failed = OS 속성 창을 열지 못했습니다: { $reason }
shell-system-menu-failed = Windows 메뉴를 띄우지 못했습니다: { $reason }
shell-share-failed = 공유하지 못했습니다: { $reason }
shell-batch-path-refused = 이 경로의 배치 파일은 관리자 권한으로 실행할 수 없습니다. 경로에 % ! ^ & ( ) @ ; , = 같은 글자가 있으면 명령 프롬프트가 경로를 명령으로 읽습니다
shell-link-not-folder = 링크가 폴더를 가리키지 않습니다
shell-link-not-file = 링크가 파일을 가리키지 않습니다
shell-no-apps = 이 파일을 열 수 있는 앱이 없습니다

## 클립보드와 붙여넣기

clipboard-unsupported = 이 시스템에서는 클립보드에 글자를 넣을 수 없습니다
clipboard-unavailable = 클립보드를 쓸 수 없습니다. 다른 앱이 쓰고 있다면 잠시 뒤에 다시 해 보세요
clipboard-not-emptied = 클립보드를 비우지 못했습니다. 다른 앱에서 붙여넣으면 잘라낸 항목이 이동될 수 있습니다
paste-nothing = 클립보드에 붙여넣을 파일이나 폴더가 없습니다
paste-cut-cancelled = 잘라내기를 취소해 붙여넣을 항목이 없습니다
paste-all-skipped = 클립보드의 항목 { $count }개가 모두 붙여넣을 수 없는 경로라 붙여넣지 않았습니다
paste-into-itself = 폴더를 그 폴더 안에 붙여넣을 수 없습니다
paste-already-here = 잘라낸 항목이 이미 이 폴더에 있습니다
paste-moving = 잘라낸 항목을 옮기는 중입니다
paste-left-out = 클립보드의 항목 { $count }개는 붙여넣을 수 없는 경로라 건너뛰었습니다

## 연결 프로그램의 메뉴

# 시스템의 기본 앱.
shell-app-default = { $name } (기본)
