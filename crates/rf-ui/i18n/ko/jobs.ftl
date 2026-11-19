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

## 끝난 작업의 알림 (words::items_done, words::trash_done, words::job_stopped, words::undone)
#
# job-items-done과 job-items-partly-done의 $work에는 drag도 온다: 끌어 놓아 옮긴 것(옮겼습니다).

job-items-done =
    항목 { $count }개를 { $work ->
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
        [drag] 옮겼습니다
       *[other] 작업을 마쳤습니다
    }
job-items-skipped = 항목 { $count }개를 건너뛰었습니다
# 충돌 규칙이 일부를 건너뛰었을 때. $count: 거친 항목 모두, $done: 한 것, $skipped: 건너뛴 것.
job-items-partly-done =
    항목 { $count }개 중 { $done }개를 { $work ->
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
        [drag] 옮겼습니다
       *[other] 작업을 마쳤습니다
    } · { $skipped }개 건너뜀
# 휴지통으로 보내야 할 항목을 위치가 영구 삭제했을 때.
job-trash-all-gone = 항목 { $count }개가 휴지통에 들어가지 않고 영구 삭제되었습니다
job-trash-some-gone = 항목 { $count }개를 휴지통으로 보냈고, { $gone }개는 휴지통 대신 영구 삭제되었습니다
job-items-failed = 항목 { $count }개를 처리하지 못했습니다
# 알림 뒤에 " · "로 붙는다.
job-gone-instead = { $count }개는 휴지통 대신 영구 삭제되었습니다
job-undone =
    { $work ->
        [copy] 복사를 되돌렸습니다
        [move] 이동을 되돌렸습니다
        [trash] 휴지통으로 보내기를 되돌렸습니다
        [delete] 삭제를 되돌렸습니다
        [rename] 이름 바꾸기를 되돌렸습니다
        [create] 만들기를 되돌렸습니다
        [link] 링크 만들기를 되돌렸습니다
        [restore] 복원을 되돌렸습니다
        [undo] 되돌리기를 되돌렸습니다
        [redo] 다시 실행을 되돌렸습니다
       *[other] 작업을 되돌렸습니다
    }
# 이름 바꾸기와 새로 만들기가 실패했을 때. $reason: 까닭(error-*, failure-*). $kind: 새로 만든 것(file, folder).
job-create-refused = { $kind ->
        [file] 새 파일을
       *[folder] 새 폴더를
    } 만들지 못했습니다: { $reason }
job-rename-refused = 이름을 바꾸지 못했습니다: { $reason }
job-failures-title = 처리하지 못한 항목

## 멈춘 작업 (words::job_stopped: 조각을 " · "로 잇는다, words::job_stopped_note)

job-stopped =
    { $work ->
        [copy] 복사를 중단했습니다
        [move] 이동을 중단했습니다
        [trash] 휴지통으로 보내기를 중단했습니다
        [delete] 삭제를 중단했습니다
        [rename] 이름 바꾸기를 중단했습니다
        [create] 만들기를 중단했습니다
        [link] 링크 만들기를 중단했습니다
        [restore] 복원을 중단했습니다
        [undo] 되돌리기를 중단했습니다
        [redo] 다시 실행을 중단했습니다
       *[other] 작업을 중단했습니다
    }
job-stopped-failed = 실패 { $count }개
job-stopped-left = 남은 항목 { $count }개는 처리하지 않았습니다
job-stopped-left-unknown = 남은 항목은 처리하지 않았습니다
# 처리하지 못한 항목 창의 첫 줄: 마지막 항목에서 멈췄을 때.
job-stopped-note-last =
    { $work ->
        [copy] 복사를 중단했습니다. 아래 항목에서 멈췄습니다.
        [move] 이동을 중단했습니다. 아래 항목에서 멈췄습니다.
        [trash] 휴지통으로 보내기를 중단했습니다. 아래 항목에서 멈췄습니다.
        [delete] 삭제를 중단했습니다. 아래 항목에서 멈췄습니다.
        [rename] 이름 바꾸기를 중단했습니다. 아래 항목에서 멈췄습니다.
        [create] 만들기를 중단했습니다. 아래 항목에서 멈췄습니다.
        [link] 링크 만들기를 중단했습니다. 아래 항목에서 멈췄습니다.
        [restore] 복원을 중단했습니다. 아래 항목에서 멈췄습니다.
        [undo] 되돌리기를 중단했습니다. 아래 항목에서 멈췄습니다.
        [redo] 다시 실행을 중단했습니다. 아래 항목에서 멈췄습니다.
       *[other] 작업을 중단했습니다. 아래 항목에서 멈췄습니다.
    }
# 그 뒤에 $count개가 남았을 때.
job-stopped-note-left =
    { $work ->
        [copy] 복사를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 복사를 다시 시작하세요.
        [move] 이동을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 이동을 다시 시작하세요.
        [trash] 휴지통으로 보내기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 휴지통으로 보내기를 다시 시작하세요.
        [delete] 삭제를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 삭제를 다시 시작하세요.
        [rename] 이름 바꾸기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 이름 바꾸기를 다시 시작하세요.
        [create] 만들기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 만들기를 다시 시작하세요.
        [link] 링크 만들기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 링크 만들기를 다시 시작하세요.
        [restore] 복원을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 복원을 다시 시작하세요.
        [undo] 되돌리기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 되돌리기를 다시 시작하세요.
        [redo] 다시 실행을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 다시 실행을 다시 시작하세요.
       *[other] 작업을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목 { $count }개는 처리하지 않았습니다. 마저 하려면 작업을 다시 시작하세요.
    }
# 남은 항목이 몇 개인지 모를 때.
job-stopped-note =
    { $work ->
        [copy] 복사를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 복사를 다시 시작하세요.
        [move] 이동을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 이동을 다시 시작하세요.
        [trash] 휴지통으로 보내기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 휴지통으로 보내기를 다시 시작하세요.
        [delete] 삭제를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 삭제를 다시 시작하세요.
        [rename] 이름 바꾸기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 이름 바꾸기를 다시 시작하세요.
        [create] 만들기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 만들기를 다시 시작하세요.
        [link] 링크 만들기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 링크 만들기를 다시 시작하세요.
        [restore] 복원을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 복원을 다시 시작하세요.
        [undo] 되돌리기를 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 되돌리기를 다시 시작하세요.
        [redo] 다시 실행을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 다시 실행을 다시 시작하세요.
       *[other] 작업을 중단했습니다. 아래 항목에서 멈췄고 그 뒤의 항목은 처리하지 않았습니다. 마저 하려면 작업을 다시 시작하세요.
    }

## 오류 때 묻는 창 (XFER-09, 파일마다 묻기)

error-ask-title =
    { $work ->
        [copy] 복사
        [move] 이동
        [trash] 휴지통으로 보내기
        [delete] 영구 삭제
        [rename] 이름 바꾸기
        [create] 새로 만들기
        [link] 링크 만들기
        [restore] 복원
        [undo] 되돌리기
        [redo] 다시 실행
       *[other] 작업
    } 중 오류가 났습니다
# 작업이 아직 답을 기다리는 동안의 까닭: 건너뛰었다고 하지 않는다(failure-*의 짝).
error-ask-too-deep = 폴더 안 너무 깊은 곳에 있어 처리할 수 없습니다
error-ask-not-a-regular-file = 장치나 파이프 같은 특수 파일이라 처리할 수 없습니다
error-ask-source-cut-short = 복사하는 동안 원본 파일이 줄어들었습니다
error-ask-onto-itself = 대상 폴더가 원본이 있는 바로 그 폴더입니다
error-ask-changed-while-moved = 옮기는 동안 항목이 바뀌었습니다
error-ask-other-volume = 다른 드라이브의 항목이라 함께 옮길 수 없습니다
error-ask-retry-once = 다시 시도는 한 번에 한 항목만 할 수 있습니다
# 충돌을 묻는 작업에 아직 남은 항목.
job-items-left = 이 작업에 남은 항목 { $count }개

## 작업이 진행 중일 때 끝내기, 지난번에 끝나지 않은 작업

quit-title = 끝낼까요?
quit-button = 끝내기
quit-with-jobs =
    진행 중인 작업 { $count }개가 있습니다.
    끝내면 다음 실행 때 이어서 할 수 있습니다.
quit-closing = 작업을 멈추고 있습니다…
recovery-title = 지난번에 끝나지 않은 작업이 { $count }개 있습니다
# 한 줄에 한 문장: 좁은 창에서 줄이 낱말 사이에서 바뀌게.
recovery-note =
    다시 실행하면 처음부터 다시 합니다.
    이미 끝난 항목은 건너뛰거나 번호를 붙인 사본이 생길 수 있습니다.
    이미 옮긴 항목은 찾을 수 없다고 나올 수 있습니다.
    닫으면 다음 실행 때 다시 묻습니다.
# 한 번도 시작하지 않아 아무것도 바꾸지 않은 작업의 줄 끝.
recovery-not-started = (시작 전)
