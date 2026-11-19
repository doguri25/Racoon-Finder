# 정리: 제거 프로그램이 부르는 --cleanup과 --cleanup-plan의 줄 (INST-04), macOS의 완전 삭제….

## --cleanup과 --cleanup-plan

cleanup-portable = 포터블 설치는 폴더를 지우면 됩니다.
cleanup-still-running = 앱이 아직 실행 중입니다. 앱을 끝낸 뒤 다시 시도하세요.
cleanup-runs-from-owned = 이 프로그램이 지울 폴더 안에서 실행 중이라 아무것도 지우지 않았습니다: { $folder }. 앱을 다른 폴더에 설치한 뒤 다시 시도하세요.
cleanup-plan-empty = 지울 것이 없습니다.
cleanup-step-backup = 설정 백업: { $from } → { $to }
# $change: 기록된 시스템 변경.
cleanup-step-revert = 시스템 변경 되돌리기: { $change }
# $domain: macOS의 환경설정 도메인.
cleanup-step-clear-domain = 환경설정 지우기: { $domain }
cleanup-step-delete = 삭제: { $path }
cleanup-step-remove-if-empty = 비어 있으면 삭제: { $path }
cleanup-step-list-temp = 임시 폴더에서 앱의 파일 찾기: { $path }
cleanup-step-other = 그 밖의 정리 단계
cleanup-backup-done = 설정을 백업했습니다: { $path }
cleanup-backup-none = 백업할 설정 파일이 없습니다.
cleanup-backup-failed = 설정을 백업하지 못해 아무것도 지우지 않았습니다: { $path } ({ $reason })
cleanup-failed = 정리하지 못했습니다: { $reason }
cleanup-reverted = 되돌린 시스템 변경: { $count }개
cleanup-revert-failed = 되돌리지 못함: { $change } ({ $reason })
cleanup-deletes-skipped = 시스템 변경을 되돌리지 못해 아무것도 지우지 않았습니다. 다시 실행하면 남은 것부터 이어서 합니다.
# $step: 위의 cleanup-step-* 하나.
cleanup-done-line = 완료 · { $step }
cleanup-remaining-line = 남음 · { $step } ({ $reason })
cleanup-complete = 정리를 마쳤습니다.
cleanup-partial = 일부를 정리하지 못했습니다.
cleanup-usage = 사용법: --cleanup [--backup-settings <파일>] | --cleanup-plan [--backup-settings <파일>]

## 완전 삭제… (macOS, 보기 ▸ 설정). $app: 앱의 이름.

removal-blocked-not-in-bundle = 이 앱은 앱 번들로 실행 중이 아니라 휴지통으로 옮길 수 없어 아무것도 지우지 않았습니다.
removal-blocked-translocated = Gatekeeper가 앱을 임시 위치에서 실행 중이라 휴지통으로 옮길 수 없어 아무것도 지우지 않았습니다. 앱을 응용 프로그램 폴더로 옮긴 뒤 다시 실행해 주세요.
removal-blocked-read-only-volume = 앱이 읽기 전용 디스크(마운트한 디스크 이미지 등)에 있어 휴지통으로 옮길 수 없어 아무것도 지우지 않았습니다. 앱을 응용 프로그램 폴더로 복사해 그 앱으로 다시 실행해 주세요.
removal-blocked-folder-not-writable = 앱이 있는 폴더에 쓸 권한이 없어 휴지통으로 옮길 수 없어 아무것도 지우지 않았습니다. 관리자에게 맡기거나 직접 휴지통으로 옮겨 주세요.
removal-save-pending = 설정 저장이 끝나지 않아 설정 폴더는 지우지 않았습니다. 잠시 뒤 앱을 끝내고 다시 실행해 지워 주세요.
removal-step-bundle = 앱을 휴지통으로 옮기기: { $path }
removal-menu = { $app } 완전 삭제…
removal-title = { $app } 완전 삭제
removal-message = 이 컴퓨터에서 { $app }를 지웁니다. 아래 설정·캐시·로그와 앱이 기록한 시스템 변경이 사라지고, 마지막으로 앱을 휴지통으로 옮깁니다(옮길 수 없으면 알려 드립니다). 지운 데이터는 되돌릴 수 없습니다.
removal-backup-label = 문서 폴더에 설정 백업
# $file: 백업 파일의 이름.
removal-backup-note = 설정 백업을 켜면 지우기 전에 문서 폴더의 { $file }로 복사합니다. 같은 이름의 파일이 있으면 덮어쓰지 않고 다른 이름으로 저장하며, 백업하지 못하면 아무것도 지우지 않습니다.
removal-button = 완전 삭제
removal-jobs-title = 작업이 진행 중입니다
removal-with-jobs =
    진행 중인 작업 { $count }개가 있습니다.
    지금 지우면 이 작업은 멈추고 다시 이어 할 수 없습니다. 작업이 끝나기를 기다리려면 취소하세요.
removal-jobs-button = 작업을 멈추고 삭제
removal-plan-failed = 지울 목록을 만들지 못했습니다: { $reason }
removal-removing = { $app }를 지우는 중…
removal-stopped-title = 완전 삭제를 마치지 못했습니다
removal-stopped-note = 여기서 멈췄습니다. 아래에 어디까지 했는지 적었습니다. 지우지 못한 것이 있으면 앱을 끝낸 뒤 다시 실행해 이어서 지울 수 있습니다.
removal-not-trashed =
    앱의 데이터는 모두 지웠지만 앱을 휴지통으로 옮기지 못했습니다: { $reason }
    앱을 끝낸 뒤 직접 휴지통으로 옮겨 주세요.
removal-backup-done = 설정을 백업했습니다: { $path }
removal-uninstaller-failed = 제거 프로그램을 열지 못했습니다: { $reason }
