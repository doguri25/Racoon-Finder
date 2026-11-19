# 속성 창 (OPS-06): 사실의 이름, 크기와 내용 세기, 체크섬.

## 사실의 이름 (차례대로)

fact-kind = 종류
fact-place = 위치
fact-size = 크기
fact-contents = 내용
fact-link-target = 링크 대상
fact-created = 만든 날짜
fact-modified = 수정한 날짜
fact-accessed = 접근한 날짜
fact-attributes = 속성
properties-reading = 읽는 중…
properties-unknown-time = 알 수 없음

## 종류와 내용, 크기. $files, $folders, $count: 세 자리마다 쉼표로 묶은 수(12,345).

properties-files = 파일 { $files }개
properties-folders = 폴더 { $folders }개
properties-files-and-folders = 파일 { $files }개, 폴더 { $folders }개
# 아직 다 세지 못한 크기. $size: 크기(size-exact).
properties-size-at-least = { $size } 이상
properties-counting = 세는 중…
properties-count-stopped = 세다가 멈췄습니다
properties-count-failed = 크기를 끝까지 세지 못했습니다
properties-unreadable-folders = 읽을 수 없는 폴더 { $count }개는 세지 못했습니다
properties-folders-elsewhere = 다른 드라이브의 폴더 { $count }개는 세지 않았습니다
properties-folder-unreadable = 폴더를 읽을 수 없습니다: { $reason }

## 속성: ", "로 잇는다.

attribute-read-only = 읽기 전용
attribute-hidden = 숨김
attribute-system = 시스템
attribute-archive = 보관
# 고른 항목 가운데 일부만 가진 속성. $name: 위의 이름 하나.
attribute-partly = { $name } (일부)
attribute-none = 없음

## 체크섬

hash-stopped = 멈춤
hashes-stopped = 계산을 멈췄습니다. 같은 알고리즘을 다시 고르면 남은 파일을 계산합니다
hash-waiting = 대기 중
hash-not-a-file = 일반 파일이 아니라 계산하지 않았습니다
hash-unreadable = 계산하지 못했습니다: { $reason }
hash-online-only = 온라인 전용 파일이라 내려받지 않았습니다
hash-only-files = 체크섬은 파일만 계산합니다
hash-choose-algorithm = 알고리즘을 고르면 계산합니다
hash-progress = 계산 중… 파일 { $done }/{ $total }
hash-first-files = 체크섬은 처음 파일 { $count }개만 계산합니다

## 붙여 넣은 해시와 비교하기

hash-match = 일치
hash-match-named = 일치: { $name }
hash-match-files = 일치: 파일 { $count }개
hash-mismatch = 불일치
hash-waiting-to-compare = 계산이 끝나면 비교합니다
