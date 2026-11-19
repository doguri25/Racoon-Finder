# 오류와 거절: 시스템의 오류, 작업이 항목을 거절한 까닭, 대상 위치에 맞지 않는 이름과 크기.

## 시스템이나 위치가 낸 오류의 종류 (words::error_kind)

error-not-found = 찾을 수 없습니다
error-permission-denied = 권한이 없습니다
error-already-exists = 같은 이름의 항목이 이미 있습니다
error-invalid-path = 쓸 수 없는 이름이나 경로입니다
error-not-a-directory = 경로에 폴더가 아닌 항목이 있습니다
error-is-a-directory = 파일이 아니라 폴더입니다
error-directory-not-empty = 폴더가 비어 있지 않습니다
error-storage-full = 디스크 공간이 부족합니다
error-read-only = 읽기 전용 위치입니다
error-crosses-devices = 다른 드라이브로는 한 번에 옮길 수 없습니다
error-busy = 다른 프로그램이 사용 중입니다
error-timed-out = 응답이 너무 늦어 중단했습니다
error-interrupted = 작업이 중간에 끊겼으니 다시 해 보세요
error-file-too-large = 파일이 너무 커서 이 위치에 둘 수 없습니다
error-unsupported = 이 항목이나 위치에서는 할 수 없는 작업입니다
error-cancelled = 취소되었습니다
error-other = 알 수 없는 오류가 났습니다

# $reason: 위의 오류 하나.
error-cannot-open = 열 수 없습니다: { $reason }
error-cannot-transfer = 복사하거나 옮길 수 없습니다: { $reason }

## 작업이 항목을 거절한 까닭 (words::failure_reason)

failure-into-itself = 폴더를 자기 안으로 복사하거나 옮길 수 없습니다
failure-no-target-folder = 대상이 드라이브 자체라 그 자리에 둘 수 없습니다
failure-target-not-a-folder = 대상 위치가 폴더가 아닙니다
failure-too-deep = 폴더 안 너무 깊은 곳에 있어 건너뛰었습니다
failure-link-loop = 바로 가기가 자기를 담은 폴더를 다시 가리켜 따라갈 수 없습니다
failure-too-many-links = 바로 가기가 너무 여러 번 이어져 따라갈 수 없습니다
failure-link-changed = 따라가는 동안 바로 가기가 바뀌었습니다
failure-absolute-link = 이 위치의 바로 가기는 따라갈 수 없습니다
failure-not-a-regular-file = 장치나 파이프 같은 특수 파일이라 건너뛰었습니다
failure-source-cut-short = 복사하는 동안 원본 파일이 줄어들어 사본을 만들지 않았습니다
failure-kind-mismatch = 같은 이름의 항목이 있는데, 파일과 폴더는 서로 덮어쓸 수 없습니다
failure-no-free-name = 번호를 붙인 이름도 모두 쓰이고 있습니다
failure-onto-itself = 대상 폴더가 원본이 있는 바로 그 폴더라 옮기지 않았습니다
failure-changed-while-moved = 옮기는 동안 항목이 바뀌어 그대로 두었습니다
failure-kept-as-target-folder = 원본 폴더가 대상 폴더와 같은 폴더라 지우지 않았습니다
failure-cannot-tell-own-folder = 원본 폴더가 대상 폴더와 같은지 알 수 없어 지우지 않았습니다
failure-other-volume = 다른 드라이브의 항목이라 건너뛰었습니다
failure-trash-refused = 휴지통으로 보낼 수 없습니다
failure-deleted-not-trashed = 휴지통 대신 영구 삭제되어 되살릴 수 없습니다
failure-changed-while-deleted = 지우는 동안 항목이 바뀌어 그대로 두었습니다
failure-overwritten = 덮어쓴 파일의 예전 내용은 되살릴 수 없습니다
failure-changed-since-copy = 복사한 뒤 바뀌어 되돌리지 않았습니다
failure-copy-partly-deleted = 복사본의 일부를 이미 지웠을 수 있어 나머지는 안전하게 비교할 수 없으므로 남겨 두었습니다
failure-changed-since-created = 만든 뒤 다른 종류로 바뀌어 되돌리지 않았습니다
failure-not-empty = 만든 뒤 내용이 생겨 되돌리지 않았습니다
failure-trash-location-unknown = 휴지통의 어디에 들어갔는지 알 수 없어 되돌릴 수 없습니다
failure-across-locations = 이름 바꾸기로는 다른 위치로 옮길 수 없습니다
failure-link-not-local = 링크는 이 컴퓨터의 드라이브에서만 만들 수 있습니다
failure-link-needs-privilege = 심볼릭 링크를 만들려면 개발자 모드를 켜거나 관리자 권한으로 실행해야 합니다
failure-link-target-missing = 링크가 가리키는 대상이 없습니다
failure-changed-since-linked = 링크가 바뀌었거나 다른 항목으로 대체되어 되돌리지 않았습니다
failure-temp-left-behind = 임시 파일을 지우지 못해 남아 있습니다
failure-trash-item-changed = 휴지통이 바뀌었습니다. 목록을 새로 고친 뒤 다시 해 주세요.
failure-original-folder-gone = 원래 폴더가 없습니다. 위치를 골라 꺼내기…로 다른 폴더에 꺼낼 수 있습니다.
failure-restore-never-replaces = 복원은 그 자리에 있는 항목을 덮어쓰지 않습니다
failure-unknown = 알 수 없는 이유로 처리하지 못했습니다

## 대상 위치에 맞지 않는 항목: 사실 (words::issue_kind)

# $char: 따옴표에 넣은 글자(‘:’)나 그 코드(U+0001).
issue-invalid-character = 대상 위치가 허용하지 않는 { $char } 문자가 이름에 있습니다
issue-reserved-name = Windows가 장치 이름으로 쓰는 이름입니다(CON, NUL 등)
issue-trailing-dot-or-space = 이름이 마침표나 공백으로 끝납니다
issue-name-too-long = 이름이 너무 깁니다
issue-path-too-long = 경로가 너무 깁니다
# $size, $max: 크기(올림, 내림).
issue-file-too-large = 파일이 대상 위치가 담을 수 있는 크기보다 큽니다 ({ $size }, 최대 { $max })
# $other: 이미 있는 항목의 이름.
issue-case-collision = 대소문자만 다른 ‘{ $other }’ 항목이 이미 있습니다
# $needed, $available: 크기(올림, 내림).
issue-not-enough-space = 대상 드라이브의 공간이 부족합니다 (필요 { $needed }, 남은 공간 { $available })
issue-unknown = 대상 위치에 맞지 않는 항목입니다

# 사실($issue) 뒤에 그것이 항목에 뜻하는 바($consequence).
issue-and-consequence = { $issue }. { $consequence }

## 작업이 그 까닭으로 이름이나 항목을 거절했을 때 (words::failure_reason)

refused-name-too-long = 더 짧게 바꿔 주세요
refused-path-too-long = 더 얕은 폴더나 짧은 이름을 써 주세요
refused-name = 이 이름은 쓸 수 없습니다
refused-no-space = 공간을 비우거나 다른 위치를 고르세요
refused-other = 이 위치에는 둘 수 없습니다

## 전송이 쓰기 전에 보여 주는 검토 (words::preflight_issue)

preflight-no-space = 계속하면 공간이 모자라 중간에 멈출 수 있습니다
preflight-warning = 그대로 두면 Windows에서 열기 어려울 수 있습니다
preflight-blocked = 이 항목은 대상 위치에 둘 수 없습니다

## 검토 창 (words::preflight_title, words::preflight_line)

preflight-title = 대상 위치에 맞지 않는 항목이 있습니다
# $name: 항목의 이름, $issue: 사실(issue-*), $follows: 뒤따르는 것(preflight-*).
preflight-line =
    { $name }: { $issue }.
    { $follows }
