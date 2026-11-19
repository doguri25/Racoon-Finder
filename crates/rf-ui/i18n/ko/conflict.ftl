# 충돌 창: 대상에 같은 이름의 항목이 있을 때 (XFER-06).
#
# $kind: file(파일), folder(폴더), item(링크나 그 밖의 항목). $work: 작업이 하는 일(jobs.ftl).

conflict-title-file = 같은 이름의 파일이 이미 있습니다
conflict-title-folder = 같은 이름의 폴더가 이미 있습니다
conflict-title-item = 같은 이름의 항목이 이미 있습니다
# $folder: 대상 폴더의 경로. $ko-final: $name을 읽을 때 받침으로 끝나면 yes, 아니면 no.
conflict-description =
    ‘{ $name }’{ $ko-final ->
        [yes] 이
       *[no] 가
    } { $folder } 에 이미 있습니다.

## 충돌 카드의 머리

conflict-card-incoming =
    { $work ->
        [copy] 복사해 올
        [move] 옮겨 올
        [restore] 복원할
       *[other] 가져올
    } { $kind ->
        [file] 파일
        [folder] 폴더
       *[item] 항목
    }
conflict-card-existing =
    이미 있는 { $kind ->
        [file] 파일
        [folder] 폴더
       *[item] 항목
    }

## 고를 것의 설명

conflict-skip =
    이 { $kind ->
        [file] 파일은
        [folder] 폴더는
       *[item] 항목은
    } { $work ->
        [copy] 복사하지 않습니다
        [move] 옮기지 않습니다
        [restore] 휴지통에 그대로 둡니다
       *[other] 가져오지 않습니다
    }
conflict-overwrite =
    있던 파일을 { $work ->
        [copy] 복사해 올
        [move] 옮겨 올
        [restore] 복원할
       *[other] 가져올
    } 파일로 바꿉니다
conflict-overwrite-kind-mismatch = 파일과 폴더는 서로 덮어쓸 수 없습니다
# $name: 작업이 처음 시도할 번호 붙은 이름.
conflict-rename = ‘{ $name }’처럼 번호를 붙여 저장합니다
conflict-overwrite-warning = 있던 파일의 내용은 되살릴 수 없습니다.
conflict-overwrite-warning-all = 남은 충돌도 모두 덮어씁니다. 있던 파일의 내용은 되살릴 수 없습니다.
conflict-newer-overwrites =
    { $work ->
        [copy] 복사해 올
        [move] 옮겨 올
        [restore] 복원할
       *[other] 가져올
    } 파일이 더 최근이라 이 파일은 덮어씁니다
conflict-newer-skips =
    { $work ->
        [copy] 복사해 올
        [move] 옮겨 올
        [restore] 복원할
       *[other] 가져올
    } 파일이 더 최근이 아니라 이 파일은 건너뜁니다
conflict-size-differs-overwrites = 크기가 달라 이 파일은 덮어씁니다
conflict-size-same-skips = 크기가 같아 이 파일은 건너뜁니다
conflict-conditional-warning-all = 남은 충돌도 같은 조건으로 덮어씁니다. 덮어쓴 파일의 내용은 되살릴 수 없습니다.
