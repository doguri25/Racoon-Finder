# 창의 말 (I18N-01): Slint 전역 `Tr`(ui/tr.slint)의 속성 하나가 이 파일의 키 하나다
# (`Tr.command-new-folder` ↔ `command-new-folder`). 창이 만들어질 때 Rust가 채운다
# (`ui_bridge::speak`). 여기의 말은 `$os` 말고는 인자를 받지 않는다. 이름이나 수가 들어가는
# 말은 Rust가 만들어 창에 준다(screen.ftl).

## 도구 모음

toolbar-back = 뒤로
toolbar-forward = 앞으로
toolbar-up = 위로
# 뒤로·앞으로의 도움말에 그 키와 함께 붙는다.
toolbar-visits = 방문 기록
toolbar-delete = 삭제
toolbar-select = 선택
toolbar-view = 보기
toolbar-filter = 이 폴더에서 거르기

## 명령: 도구 모음과 항목·폴더의 메뉴

command-new-folder = 새 폴더
command-copy-to = 복사…
command-move-to = 이동…
command-rename = 이름 바꾸기
command-undo = 되돌리기
command-redo = 다시 실행
command-cut = 잘라내기
command-copy = 복사
command-paste = 붙여넣기
# macOS의 붙여넣기로 옮기기(⌥⌘V). 다른 시스템에는 없다.
command-paste-move = 여기로 이동
command-copy-path = 경로 복사
command-create-link = 링크 만들기
command-share = 공유…
command-new-file = 새 파일
command-new-text-file = 빈 텍스트 파일
command-open = 열기
command-run-as-admin = 관리자 권한으로 실행
command-open-with = 연결 프로그램으로 열기…
command-quick-look = 빠른 보기
command-open-terminal = 터미널에서 열기
command-add-favorite = 즐겨찾기에 추가
command-trash = 휴지통으로 보내기
command-delete-permanently = 영구 삭제
# 앱의 속성 창.
command-properties = 속성
# 시스템의 속성 창(Windows 속성, macOS Finder의 정보 가져오기).
command-system-properties = OS 속성 창 열기
# Windows의 메뉴(Shift+F10).
command-more-options = 더 많은 옵션 표시
command-restore = 복원
command-restore-to = 위치를 골라 꺼내기…
command-delete-from-trash = 영구 삭제…
command-empty-trash = 휴지통 비우기…
command-refresh = 새로 고침
# 도구 모음을 숨겼을 때 폴더의 메뉴에 나온다.
command-show-toolbar = 툴바 보이기

## 놓을 때 묻는 메뉴

drop-menu-move = 여기로 이동
drop-menu-copy = 여기로 복사
drop-menu-link = 여기에 링크 만들기
drop-menu-cancel = 취소

## 경로 복사의 형식

path-form-full = 전체 경로
path-form-name = 이름만
path-form-quoted = 따옴표 포함
path-form-unc = UNC 경로
path-form-slashes = 슬래시 형식
path-form-url = 파일 URL

## 자세히의 열 머리 (보기 메뉴의 열과 정렬, 머리의 메뉴도 같은 이름이다. Rust도 쓴다)

column-name = 이름
column-size = 크기
column-modified = 수정한 날짜
column-created = 만든 날짜
column-kind = 종류
column-extension = 확장자
column-attributes = 속성
column-accessed = 접근한 날짜
column-path = 경로
column-original = 원래 위치
column-deleted = 삭제한 날짜

## 보기 메뉴

view-mode = 보기 방식
view-thumbnails = 썸네일
view-extra-large-icons = 특대 아이콘
view-large-icons = 큰 아이콘
view-medium-icons = 보통 아이콘
view-small-icons = 작은 아이콘
view-list = 목록
view-details = 자세히
view-tiles = 타일
view-thumb-size = 썸네일 크기 조절
view-apply-to-subfolders = 하위 폴더에 적용
view-reset-to-default = 기본값으로 되돌리기
view-sort = 정렬
view-ascending = 오름차순
view-descending = 내림차순
view-then-sort = 2차 정렬
view-then-none = 없음
view-folders-first = 폴더 먼저
view-columns = 열
view-column-order = 열 순서
view-column-left = 왼쪽으로
view-column-right = 오른쪽으로
view-fit-columns = 모든 열 너비 맞추기
view-theme = 테마
view-theme-light = 밝게
view-theme-dark = 어둡게
view-theme-high-contrast = 고대비
view-theme-system = 시스템 설정 따르기
view-density-compact = 표시 밀도: 촘촘하게
view-density-normal = 표시 밀도: 보통
view-density-roomy = 표시 밀도: 넉넉하게
view-text-size-90 = 글자 크기: 90%
view-text-size-100 = 글자 크기: 100%
view-text-size-115 = 글자 크기: 115%
view-text-size-130 = 글자 크기: 130%
view-show-hidden = 숨김 파일 보기
view-show-extensions = 파일 확장자 보기
view-toolbar = 툴바
view-tree-shared = 공유 트리
view-tree-per-pane = 패널별 트리
view-tree-hidden = 트리 숨김
view-focus-tree = 폴더 트리로 가기
view-preview = 미리 보기 패널
view-job-panel = 작업 패널
view-jobs-window = 별도 창
view-layout = 배치
view-dual-pane = 듀얼 패널
view-split = 위아래로 나누기
view-even-split = 두 패널을 같은 크기로
view-divider-up = 분할선을 위로
view-divider-left = 분할선을 왼쪽으로
view-divider-down = 분할선을 아래로
view-divider-right = 분할선을 오른쪽으로
view-swap-panes = 패널 바꾸기
view-same-folder = 같은 폴더를 반대편에
view-sync = 탐색 동기화
view-tab = 탭
view-tab-duplicate = 탭 복제
view-tab-unpin = 탭 고정 풀기
view-tab-pin = 탭 고정
view-tab-move-other = 탭을 반대 패널로 옮기기
view-tab-move-left = 탭을 왼쪽으로
view-tab-move-right = 탭을 오른쪽으로
view-settings = 설정
view-drop-move = 끌어 놓기: 이동
view-drop-copy = 끌어 놓기: 복사
view-drop-system = 끌어 놓기: OS 방식
view-drop-ask = 끌어 놓기: 매번 묻기
view-show-thumbnails = 썸네일 보이기
view-long-press = 길게 눌러 메뉴 열기
view-long-press-300 = 길게 누르기 시간: 300 ms
view-long-press-500 = 길게 누르기 시간: 500 ms
view-long-press-800 = 길게 누르기 시간: 800 ms
view-long-press-1200 = 길게 누르기 시간: 1200 ms
view-move-undo-toast = 끌어서 옮긴 뒤 되돌리기 알림
view-ask-on-error = 복사·이동 오류는 파일마다 묻기
view-pick-from-last = 복사·이동 창을 최근 위치의 첫 폴더로 열기
# Windows에만 있다.
view-system-menu-first = 오른쪽 클릭에 Windows 메뉴 바로 띄우기
# 설치한 앱의 Windows에만 있다.
view-open-uninstaller = 제거 프로그램 열기

## 선택 메뉴

select-all = 모두 선택
select-none = 선택 해제
select-invert = 선택 반전
select-pattern = 패턴으로 선택…
select-same-extension = 같은 확장자 선택
# 보기 ▸ 설정에도 있다.
select-checkboxes = 체크박스로 선택

## 패널

pane-left = 왼쪽 패널
pane-right = 오른쪽 패널
# 경로 줄과 경로를 고치는 칸의 이름.
pane-path = 경로
pane-loading = 불러오는 중…
pane-cannot-open = 이 폴더를 열 수 없습니다
pane-retry = 다시 시도
# 썸네일 보기의 크기 막대.
pane-thumb-size = 썸네일 크기
# 경로 조각의 ›에 뜨는 도움말.
pane-subfolders-tip = 하위 폴더 보기
# ›가 여는 하위 폴더 목록과 경로 칸의 자동 완성 목록의 이름.
pane-subfolders = 하위 폴더
pane-path-completion = 경로 자동 완성
# 하위 폴더 목록에서 링크인 폴더.
pane-link = 바로 가기
list-items = 항목 목록
list-select-all = 모든 항목 선택
# 화면 읽기 프로그램이 이름 뒤에 듣는 말.
row-cut = 잘라낸 항목

## 탭

tab-pinned = 고정된 탭
tab-close-others = 다른 탭 닫기
tab-close-right = 오른쪽 탭 닫기
tab-duplicate = 복제
tab-unpin = 고정 풀기
tab-pin = 고정
tab-move-other = 반대 패널로 옮기기
tab-reopen = 닫은 탭 다시 열기
tab-close-button = 탭 닫기
tab-new = 새 탭
# 탭의 메뉴와 빠른 보기의 닫기.
common-close = 닫기

## 사이드바

tree-title = 폴더 트리
sidebar-open-other = 반대 패널에 열기
sidebar-remove-favorite = 즐겨찾기에서 제거
sidebar-move-up = 위로
sidebar-move-down = 아래로
sidebar-add-current = 현재 폴더를 즐겨찾기에 추가
sidebar-add-this = 이 폴더를 즐겨찾기에 추가
sidebar-favorites = 즐겨찾기
sidebar-this-pc = 이 PC
# 즐겨찾기 사이에 끌어 놓을 자리의 표시.
sidebar-insert-here = 끼워 넣을 자리

## 미리 보기와 알림

preview-title = 미리 보기
preview-nothing = 미리 볼 항목이 없습니다
toast-close = 알림 닫기

## 트레이 아이콘의 메뉴

tray-open-window = 창 열기
tray-resume-all = 모두 계속
tray-pause-all = 모두 일시정지
tray-quit = 끝내기

## 보기 ▸ 테마의 언어 (다시 시작하면 바뀐다). 언어의 이름은 어느 언어로든 그 언어로 쓴다.

view-language-system = 언어: 시스템 설정 따르기
view-language-ko = 언어: 한국어
view-language-en = 언어: English

## 대화상자들이 함께 쓰는 단추

common-cancel = 취소
common-ok = 확인
common-continue = 계속

## 이름이 겹칠 때 할 일: 충돌 창의 선택과 위치 고르기 창의 정책

policy-overwrite = 덮어쓰기
policy-skip = 건너뛰기
policy-if-newer = 더 새 파일만 덮어쓰기
policy-if-size-differs = 크기가 다를 때만 덮어쓰기

## 이름 충돌 창

conflict-newer-badge = 더 최근
conflict-size-label = 크기
conflict-modified-label = 수정한 날짜
conflict-question = 어떻게 할까요?
conflict-choice-keep-both = 이름 바꿔 둘 다 두기
conflict-apply-all = 남은 충돌에도 같은 선택 적용
conflict-cancel-job = 작업 취소

## 위치 고르기 창

# 이동/복사 고르기의 이름.
pick-mode = 작업
pick-mode-move = 이동
pick-mode-copy = 복사
pick-target = 대상 경로
pick-target-hint = 폴더 경로를 입력하세요
pick-filter = 폴더 이름으로 거르기
pick-tree = 대상 폴더 트리
pick-quick = 빠른 대상
pick-on-conflict = 이름이 겹치면
pick-choice-ask = 물어보기
pick-choice-rename = 이름 바꾸기
# 앞의 "이름이 겹치면"과 아래 줄이 나머지를 말한다(policy-if-newer).
pick-choice-if-newer = 더 새 파일만
pick-choice-if-size-differs = 크기가 다를 때만
pick-follow-links = 링크는 가리키는 것을 복사

## 속성 창

properties-window = 속성
properties-stop = 중지
properties-checksum = 체크섬
# 체크섬 목록의 이름(화면 읽기 프로그램).
properties-checksums = 체크섬
properties-algorithm = 알고리즘
properties-copy = 복사
properties-copied = 복사함
properties-stop-hashing = 계산 중지
properties-compare = 비교
properties-compare-hint = 붙여넣은 해시와 비교

## 작업 패널과 그 창

job-panel-title = 작업
job-panel-speed = 속도 그래프
job-panel-pause = 일시정지
job-panel-resume = 계속
job-panel-cancel = 취소
# 기다리는 작업을 대기열에서 한 자리 앞으로. 화면 읽기 프로그램은 -spoken을 듣는다(도구 모음에도 위로가 있다).
job-panel-move-up = 위로
job-panel-move-up-spoken = 작업 순서 위로
job-panel-move-down = 아래로
job-panel-move-down-spoken = 작업 순서 아래로
job-panel-waiting = 대기 중인 작업
job-panel-expand = 펼치기
job-panel-collapse = 접기
job-panel-clear = 지우기
job-panel-recent = 최근 완료
job-panel-details = 자세히
job-panel-save-log = 로그 저장…
job-panel-history = 되돌리기 기록
# 되돌리기 기록에서 단추가 다음에 할 줄.
job-panel-next = 다음
job-window-empty = 진행 중인 작업이 없습니다

## 대화상자

pattern-title = 패턴으로 선택
pattern-example-wildcards = 예: *.jpg;*.png
pattern-example-regex = 예: ^IMG_\d+\.jpe?g$
pattern-field = 패턴
pattern-syntax = 패턴 형식
pattern-wildcards = 와일드카드
pattern-regex = 정규식
pattern-ignore-case = 대소문자 무시
# 맞는 항목을 선택과 어떻게 합칠지 고르기의 이름.
pattern-mode = 고른 항목
pattern-replace = 선택 바꾸기
pattern-add = 선택에 더하기
pattern-select = 선택
# 처리하지 못한 항목의 창: 실패한 항목만 다시 한다(XFER-09).
failures-retry = 다시 시도
# 파일마다 묻는 창(XFER-09).
error-ask-apply-all = 남은 오류에도 같은 선택
error-ask-stop = 중단
error-ask-skip = 건너뛰기
error-ask-retry = 다시 시도
# 지난번에 끝나지 않은 작업.
recovery-discard = 버리기
recovery-run-again = 다시 실행
preflight-skip-blocked = 막힌 항목 건너뛰기
