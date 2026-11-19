# 창의 알림과 패널·보기의 말: 폴더 목록, 보기, 패턴 선택, 탐색 동기화, 탭, 설정.

## 위치와 폴더 목록

notice-remote-location = 아직 지원하지 않는 위치입니다
# 하위 폴더 목록(›).
tree-loading = 불러오는 중…
tree-no-subfolders = 하위 폴더가 없습니다
tree-links-unexamined = 바로 가기 일부는 확인하지 않았습니다
# 주소창 목록 아래: 보이지 않는 폴더의 수.
tree-more-folders = 외 { $count }개
drop-moved = 목록이 바뀌어 놓을 곳이 달라졌습니다. 다시 끌어다 놓으세요
notice-system-menu-moved = 목록이 바뀌어 Windows 메뉴에서 고른 명령을 하지 않았습니다. 메뉴를 다시 띄워 고르세요
# $to: parent(상위 폴더)나 home(홈 폴더).
notice-folder-gone =
    ‘{ $name }’ 폴더가 없어져 { $to ->
        [parent] 상위 폴더
       *[home] 홈 폴더
    }로 옮겼습니다
notice-folder-gone-sync-off =
    ‘{ $name }’ 폴더가 없어져 { $to ->
        [parent] 상위 폴더
       *[home] 홈 폴더
    }로 옮겼고 탐색 동기화를 껐습니다
# $path: 가운데를 줄인 경로.
notice-start-folder-gone = 열려던 폴더를 찾을 수 없습니다: { $path }
notice-background-failed = 시스템 자원이 부족해 일을 시작하지 못했습니다. 잠시 뒤에 다시 해 보세요
notice-sort-failed = 이 폴더를 정렬하지 못해 이전 순서로 둡니다

## 폴더의 보기

notice-computer-view-fixed = 내 PC는 드라이브를 타일로만 보여 줍니다
view-applied-below = ‘{ $name }’ 폴더와 그 하위 폴더를 이 보기로 표시합니다
view-apply-title = 하위 폴더에 적용
view-apply-below =
    ‘{ $name }’ 폴더의 보기를 하위 폴더에도 적용할까요?
    하위 폴더 { $count }개에 따로 정한 보기가 지워집니다.
view-apply-button = 적용
view-reset = ‘{ $name }’ 폴더의 보기를 되돌렸습니다
view-not-kept = 경로가 너무 길어 이 폴더의 보기를 기억할 수 없습니다

## 고르기

select-no-extension = ‘{ $name }’에는 확장자가 없습니다
pattern-uncounted = 항목 수를 세지 못했습니다. 선택을 누르면 바로 맞춰 고릅니다
pattern-count = 맞는 항목 { $count }개
pattern-counting = 세는 중…
pattern-too-long = 패턴이 너무 깁니다 ({ $max }자까지)
pattern-invalid = 올바른 정규식이 아닙니다
pattern-too-complex = 패턴이 너무 복잡합니다. 더 짧게 써 주세요

## 탐색 동기화, 패널, 탭

sync-on = 탐색 동기화를 켰습니다
sync-off = 탐색 동기화를 껐습니다
sync-no-folder = 반대편 패널에 ‘{ $name }’ 폴더가 없어 탐색 동기화를 껐습니다
sync-cannot-follow = 반대편 패널이 따라갈 수 없어 탐색 동기화를 껐습니다
sync-off-by-swap = 패널을 맞바꿔 탐색 동기화를 껐습니다
pane-one-only = 한 패널 보기에서는 쓸 수 없습니다
tab-last-stays = 패널의 마지막 탭은 옮길 수 없습니다
tab-moved-to-hidden = 탭을 숨은 반대편 패널로 옮겼습니다. 보기 ▸ 배치 ▸ 듀얼 패널(F12)로 볼 수 있습니다

## 설정

settings-not-saved = 설정을 저장하지 못했습니다. 다음 실행 때 탭과 보기 설정이 돌아오지 않을 수 있습니다
settings-unreadable = 설정 파일을 읽지 못해 기본 설정으로 시작했습니다. 이번에 바꾼 설정은 저장하지 않습니다
# $path: 설정 파일을 옮겨 둔 곳.
settings-backed-up = 설정 파일을 읽을 수 없어 백업하고 기본값으로 시작했습니다: { $path }
