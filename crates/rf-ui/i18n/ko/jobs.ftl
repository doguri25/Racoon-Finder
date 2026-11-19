# 작업: 작업 패널의 줄, 끝난 작업의 알림, 트레이.
#
# $work는 작업이 하는 일이다: copy, move, trash(휴지통으로), delete(영구 삭제), rename, create,
# link(링크 만들기), restore(휴지통에서 복원), undo, redo, other(이 판이 이름을 모르는 일).

## 작업의 이름 (words::JobWork::noun)

job-noun-copy = 복사
job-noun-move = 이동
job-noun-trash = 휴지통으로 보내기
job-noun-delete = 영구 삭제
job-noun-rename = 이름 바꾸기
job-noun-create = 새로 만들기
job-noun-link = 링크 만들기
job-noun-restore = 복원
job-noun-undo = 되돌리기
job-noun-redo = 다시 실행
job-noun-other = 작업

## 작업 줄의 제목 (words::job_title, words::outcome)

job-queued = 대기 중
job-paused = 일시정지됨
job-doing =
    { $work ->
        [copy] 복사 중
        [move] 이동 중
        [trash] 휴지통으로 보내는 중
        [delete] 삭제 중
        [rename] 이름 바꾸는 중
        [create] 만드는 중
        [link] 링크 만드는 중
        [restore] 복원하는 중
        [undo] 되돌리는 중
        [redo] 다시 실행하는 중
       *[other] 작업 중
    }
job-done =
    { $work ->
        [copy] 복사했습니다
        [move] 이동했습니다
        [trash] 휴지통으로 보냈습니다
        [delete] 삭제했습니다
        [rename] 이름을 바꿨습니다
        [create] 만들었습니다
        [link] 링크를 만들었습니다
        [restore] 복원했습니다
        [undo] 되돌렸습니다
        [redo] 다시 실행했습니다
       *[other] 작업을 마쳤습니다
    }
job-done-with-errors =
    { $work ->
        [copy] 복사했지만 일부 항목을 처리하지 못했습니다
        [move] 이동했지만 일부 항목을 처리하지 못했습니다
        [trash] 휴지통으로 보냈지만 일부 항목을 처리하지 못했습니다
        [delete] 삭제했지만 일부 항목을 처리하지 못했습니다
        [rename] 이름을 바꿨지만 일부 항목을 처리하지 못했습니다
        [create] 만들었지만 일부 항목을 처리하지 못했습니다
        [link] 링크를 만들었지만 일부 항목을 처리하지 못했습니다
        [restore] 복원했지만 일부 항목을 처리하지 못했습니다
        [undo] 되돌렸지만 일부 항목을 처리하지 못했습니다
        [redo] 다시 실행했지만 일부 항목을 처리하지 못했습니다
       *[other] 작업을 마쳤지만 일부 항목을 처리하지 못했습니다
    }
job-cancelled =
    { $work ->
        [copy] 복사를 취소했습니다
        [move] 이동을 취소했습니다
        [trash] 휴지통으로 보내기를 취소했습니다
        [delete] 삭제를 취소했습니다
        [rename] 이름 바꾸기를 취소했습니다
        [create] 만들기를 취소했습니다
        [link] 링크 만들기를 취소했습니다
        [restore] 복원을 취소했습니다
        [undo] 되돌리기를 취소했습니다
        [redo] 다시 실행을 취소했습니다
       *[other] 작업을 취소했습니다
    }
job-aborted =
    { $work ->
        [copy] 문제가 생겨 복사를 중단했습니다
        [move] 문제가 생겨 이동을 중단했습니다
        [trash] 문제가 생겨 휴지통으로 보내기를 중단했습니다
        [delete] 문제가 생겨 삭제를 중단했습니다
        [rename] 문제가 생겨 이름 바꾸기를 중단했습니다
        [create] 문제가 생겨 만들기를 중단했습니다
        [link] 문제가 생겨 링크 만들기를 중단했습니다
        [restore] 문제가 생겨 복원을 중단했습니다
        [undo] 문제가 생겨 되돌리기를 중단했습니다
        [redo] 문제가 생겨 다시 실행을 중단했습니다
       *[other] 문제가 생겨 작업을 중단했습니다
    }
# 이 판이 모르는 끝.
job-ended =
    { $work ->
        [copy] 복사를 끝냈습니다
        [move] 이동을 끝냈습니다
        [trash] 휴지통으로 보내기를 끝냈습니다
        [delete] 삭제를 끝냈습니다
        [rename] 이름 바꾸기를 끝냈습니다
        [create] 만들기를 끝냈습니다
        [link] 링크 만들기를 끝냈습니다
        [restore] 복원을 끝냈습니다
        [undo] 되돌리기를 끝냈습니다
        [redo] 다시 실행을 끝냈습니다
       *[other] 작업을 끝냈습니다
    }

## 작업 줄의 진행 (words::job_progress, words::time_left)

job-planning = 준비 중
job-waiting = 답을 기다리는 중
job-progress-items = { $done } / { $total } 항목
# $speed: 크기(48 MB).
job-speed = { $speed }/s
# $time: duration-* 하나.
job-time-left = 남은 시간 약 { $time }
job-progress-queued = 대기 중인 작업 { $count }개

## 작업 패널의 창 (words::job_window_title)

job-window-title = 작업 - { $app }
job-window-failed = 작업 패널 창을 열지 못해 창 아래에 다시 붙였습니다

## 최근 완료의 줄 (words::job_report): 이 조각들을 " · "로 잇는다.

job-report-done =
    { $work ->
        [copy] 복사
        [move] 이동
        [trash] 휴지통
        [delete] 삭제
        [rename] 이름 바꿈
        [create] 만듦
        [link] 링크 만듦
        [restore] 복원
        [undo] 되돌림
        [redo] 다시 실행
       *[other] 처리
    } { $count }개
job-report-skipped = 건너뜀 { $count }개
job-report-failed = 오류 { $count }개
job-report-speed = 평균 { $speed }/s
job-report-took = 걸린 시간 { $time }

## 작업의 로그 (XFER-14)

job-log-saved = 로그를 저장했습니다: { $name }
job-log-failed = 로그를 저장하지 못했습니다: { $reason }
pick-log-title = 로그를 어디에 저장할까요?
pick-log-folder = 로그 폴더
pick-log-confirm = 여기에 저장
job-journal-error = 작업 기록을 저장하지 못했습니다. 작업은 계속되지만 앱이 갑자기 꺼지면 이어서 할 수 없습니다

## 창이 앞에 없을 때 끝난 작업의 시스템 알림 (words::job_notice)

job-notice-done =
    { $work ->
        [copy] 복사를 마쳤습니다 · 항목 { $count }개
        [move] 이동을 마쳤습니다 · 항목 { $count }개
        [trash] 휴지통으로 보내기를 마쳤습니다 · 항목 { $count }개
        [delete] 삭제를 마쳤습니다 · 항목 { $count }개
        [rename] 이름 바꾸기를 마쳤습니다 · 항목 { $count }개
        [create] 만들기를 마쳤습니다 · 항목 { $count }개
        [link] 링크 만들기를 마쳤습니다 · 항목 { $count }개
        [restore] 복원을 마쳤습니다 · 항목 { $count }개
        [undo] 되돌리기를 마쳤습니다 · 항목 { $count }개
        [redo] 다시 실행을 마쳤습니다 · 항목 { $count }개
       *[other] 작업을 마쳤습니다 · 항목 { $count }개
    }
job-notice-done-with-errors =
    { $work ->
        [copy] 복사를 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [move] 이동을 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [trash] 휴지통으로 보내기를 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [delete] 삭제를 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [rename] 이름 바꾸기를 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [create] 만들기를 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [link] 링크 만들기를 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [restore] 복원을 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [undo] 되돌리기를 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
        [redo] 다시 실행을 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
       *[other] 작업을 마쳤지만 항목 { $failed }개를 처리하지 못했습니다
    }

## 트레이 (XFER-15). macOS는 메뉴 막대다.

tray-all-done = 모든 작업을 마쳐 앱을 끝냈습니다.
tray-all-done-with-failures = 처리하지 못한 항목이 있어 창을 다시 열었습니다. 최근 완료의 자세히에서 볼 수 있습니다.
tray-unavailable = 트레이 아이콘을 만들지 못해 창을 닫지 않았습니다
tray-continue = 트레이에서 계속
tray-continue-menu-bar = 메뉴 막대에서 계속
tray-ask = 작업이 끝날 때까지 트레이에서 계속할까요?
tray-ask-menu-bar = 작업이 끝날 때까지 메뉴 막대에서 계속할까요?
