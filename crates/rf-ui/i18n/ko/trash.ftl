# 영구 삭제와 휴지통 (OPS-03, NAV-16): 확인 창, 알림, 휴지통과 내 PC에서 할 수 없는 일.

## 영구 삭제

delete-permanently-title = 영구 삭제
delete-permanently-button = 영구 삭제
delete-permanently =
    항목 { $count }개를 영구 삭제할까요?
    휴지통을 거치지 않으며 되돌릴 수 없습니다.

## 휴지통의 항목을 영구 삭제하기, 휴지통 비우기. $size: 모든 항목의 크기를 알 때 그 합.

trash-delete-items =
    휴지통의 항목 { $count }개를 영구 삭제할까요?
    되돌릴 수 없습니다.
trash-delete-items-sized =
    휴지통의 항목 { $count }개({ $size })를 영구 삭제할까요?
    되돌릴 수 없습니다.
trash-empty-title = 휴지통 비우기
trash-empty =
    휴지통의 항목 { $count }개를 모두 영구 삭제할까요?
    되돌릴 수 없습니다.
trash-empty-sized =
    휴지통의 항목 { $count }개({ $size })를 모두 영구 삭제할까요?
    되돌릴 수 없습니다.
trash-already-empty = 휴지통이 비어 있습니다
# $reason: 오류 하나(error-*).
trash-unreadable = 휴지통을 읽지 못했습니다: { $reason }

## 복원

trash-place-unknown = 원래 위치를 모르는 항목이 있어 복원할 수 없습니다. 위치를 골라 꺼내기…로 꺼내세요.
# $folder: 항목이 모두 들어간 폴더의 이름.
trash-restored-to = 항목 { $count }개를 ‘{ $folder }’ 폴더로 복원했습니다
trash-restored = 항목 { $count }개를 각각 원래 위치로 복원했습니다
trash-folders-gone = 원래 폴더가 없어 항목 { $count }개를 복원하지 못했습니다
trash-restore-elsewhere = 위치를 골라 꺼내기…
# 위치를 골라 꺼내기…에 여러 볼륨의 휴지통 항목을 함께 골랐을 때.
trash-restore-two-volumes = 고른 항목이 여러 드라이브(볼륨)의 휴지통에 있어 한 폴더로 꺼낼 수 없습니다. 드라이브마다 따로 골라 꺼내세요.
trash-item-not-opened = 휴지통 안의 항목은 열 수 없습니다. 먼저 복원하세요.
computer-drive-gone = 드라이브를 찾을 수 없습니다. 새로 고침(F5)으로 목록을 다시 읽으세요.

## 휴지통과 내 PC에서 할 수 없는 일 (words::shell_blocked)
#
# $place: trash(휴지통)나 computer(내 PC). $what: 할 수 없는 일.

shell-blocked =
    { $place ->
        [trash] 휴지통에서는
       *[computer] 내 PC에서는
    } { $what ->
        [new_folder] 새 폴더를 만들
        [new_file] 새 파일을 만들
        [paste] 붙여넣을
        [rename] 이름을 바꿀
        [link] 링크를 만들
        [copy] 복사할
        [cut] 잘라낼
        [transfer] 복사하거나 옮길
        [copy_path] 경로를 복사할
        [properties] 속성을 볼
        [open_with] 다른 프로그램으로 열
        [terminal] 터미널을 열
        [preview] 미리 볼
        [share] 공유할
        [delete] 삭제할
       *[favorite] 즐겨찾기에 추가할
    } 수 없습니다. { $place ->
        [trash] 항목은 복원하거나 영구 삭제할 수 있습니다.
       *[computer] 드라이브를 열면 그 안에서 할 수 있습니다.
    }

## 휴지통으로 보내기

# 사이드바의 휴지통 위로 끌 때 포인터 옆.
drop-to-trash = 휴지통으로 보내기
trash-unavailable =
    휴지통을 쓸 수 없는 위치입니다.
    항목 { $count }개를 영구 삭제할까요? 되돌릴 수 없습니다.
trash-refused =
    휴지통이 받지 않은 항목이 { $count }개 있습니다.
    이 항목을 영구 삭제할까요? 되돌릴 수 없습니다.

## 복사를 되돌릴 때 휴지통이 받지 않은 복사본

copies-refused = 복사본을 휴지통으로 보낼 수 없습니다. 원본은 그대로 있습니다.
copies-delete = 복사본 { $count }개를 영구 삭제할까요? 되돌릴 수 없습니다.
copies-deleted = 복사본 { $count }개를 영구 삭제해 복사를 되돌렸습니다
copies-were-gone = 복사본이 이미 없어 복사를 되돌렸습니다
copies-checking = 원본을 확인하는 중…
# 복사본을 지우지 않은 까닭 (words::copy_kept).
copy-kept-original-missing = 원본이 없어 복사본을 지우지 않았습니다
copy-kept-original-changed = 원본이 바뀌어 복사본을 지우지 않았습니다
copy-kept-original-unknown = 원본을 확인할 수 없어 복사본을 지우지 않았습니다
copy-kept-copy-changed = 복사본이 바뀌어 지우지 않았습니다
copy-kept-copy-missing = 복사본이 이미 없습니다
copy-kept-copy-unknown = 복사본을 확인할 수 없어 지우지 않았습니다
