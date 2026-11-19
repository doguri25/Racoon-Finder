# 위치 고르기 창 (PICK): 복사·이동할 곳, 링크를 만들 곳, 휴지통에서 꺼낼 곳.

## 복사·이동할 곳

pick-title-copy = 어디로 복사할까요?
pick-title-move = 어디로 이동할까요?
pick-confirm-copy = 여기로 복사
pick-confirm-move = 여기로 이동
# 항목 수 뒤에 " · "로 붙는다. 폴더의 크기는 작업이 안을 보기 전에는 모른다.
pick-summary-folders = 폴더 { $count }개
# $size: 파일의 크기를 더한 것. 크기의 단위는 읽을 때 모음으로 끝나 늘 '와'가 붙는다.
pick-summary-files-and-folders = 파일 { $size }와 폴더 { $count }개
# 처음 몇 항목의 이름 뒤.
pick-more = 외 { $count }개

## 빠른 대상의 머리

pick-opposite = 반대편 패널
pick-tabs = 열린 탭
pick-recent = 최근 위치
pick-favorites = 즐겨찾기

## 고를 수 없는 폴더, 조심할 것 (words::pick_problem)

pick-problem-into-copied = 복사하는 항목 안이라 고를 수 없음
pick-problem-into-moved = 옮기는 항목 안이라 고를 수 없음
pick-problem-into-linked = 링크를 만드는 항목 안이라 고를 수 없음
pick-problem-same-folder = 원래 있던 폴더라 고를 수 없음
pick-problem-not-a-folder = 폴더를 찾을 수 없어 고를 수 없음
pick-problem-shell-place = 휴지통과 내 PC에는 넣을 수 없음
# 위치를 골라 꺼내기…: 휴지통의 항목은 이름을 바꿔 꺼내므로 다른 볼륨으로 갈 수 없다.
pick-problem-other-volume = 다른 드라이브라 꺼낼 수 없음: 휴지통의 항목은 같은 드라이브(볼륨)의 폴더로만 꺼냅니다
pick-problem-not-writable = 읽기 전용 위치라 작업이 실패할 수 있습니다
# $need(올림), $available(내림): 크기.
pick-problem-space = 공간이 부족할 수 있습니다: 필요 { $need }, 남은 공간 { $available }
# 고른 것에 폴더가 있어 필요한 공간이 더 클 수 있을 때.
pick-problem-space-at-least = 공간이 부족할 수 있습니다: 필요 { $need } 이상, 남은 공간 { $available }
# $free: 크기(내림).
pick-space = 남은 공간 { $free }
pick-space-pending = 남은 공간을 확인하는 중…

## 경로 칸

pick-path-pending = 입력한 경로를 확인하는 중…
pick-path-unentered = Enter로 경로를 먼저 확인하세요
pick-path-blank = 경로를 입력하거나 폴더를 고르세요
pick-overwrite-by-button = 덮어쓰기는 버튼으로만 확인합니다
pick-path-not-a-folder = 폴더가 아닙니다

## 충돌 규칙의 설명 (words::pick_policy): 한 줄에 한 문장.

pick-policy-overwrite =
    같은 이름의 파일을 바꿉니다.
    있던 파일의 내용은 되살릴 수 없습니다.
pick-policy-if-newer =
    수정한 날짜가 더 최근일 때만 같은 이름의 파일을 바꾸고, 아니면 건너뜁니다.
    바꾼 파일의 내용은 되살릴 수 없습니다.
pick-policy-if-size-differs =
    크기가 다를 때만 같은 이름의 파일을 바꾸고, 같으면 건너뜁니다.
    바꾼 파일의 내용은 되살릴 수 없습니다.
pick-policy-skip = 같은 이름이 있는 항목은 건너뜁니다
pick-policy-auto-rename = 같은 이름이 있으면 번호를 붙여 둘 다 둡니다
pick-policy-ask = 같은 이름이 있으면 그때 묻습니다
pick-no-match = 펼쳐 본 폴더 중에 맞는 이름이 없습니다

## 링크를 만들 곳 (PICK-09), 휴지통에서 꺼낼 곳

pick-link-title = 링크를 어디에 만들까요?
pick-link-confirm = 여기에 링크 만들기
# $kind: 링크의 종류(link-kind-*).
pick-link-summary = 항목 { $count }개의 { $kind }
pick-restore-title = 위치를 골라 꺼내기
pick-restore-summary = 휴지통의 항목 { $count }개
pick-restore-confirm = 여기로 꺼내기
