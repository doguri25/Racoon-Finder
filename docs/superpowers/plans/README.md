# 구현 계획 목록

기준 문서: [`docs/PRD.md`](../../PRD.md)

## 실행 순서

| 순서 | 계획 | 내용 | 실행 위치 |
|---|---|---|---|
| 1 | [M0 기술 검증](2026-10-22-m0-tech-spike.md) | Slint 게이트 6개, 탐색기 대체·Win+E·`/select`, NSFileViewer, 복사 성능, OpenDAL, 압축 라이브러리, 색인 속도, 변경 원장 VM 검증, 설치 도구. 마지막 과제에서 결정 표를 채우고 PRD를 갱신한다. | 대부분 Windows 11·macOS 15 실기. 과제 1, 13, 17, 18, 21과 과제 19의 1~3단계는 Linux에서 가능 |
| 2 | [M1-01 워크스페이스와 rf-core](2026-10-22-m1-01-workspace-core.md) | Cargo 워크스페이스, CI, cargo-deny, `VPath`·`Entry`·`AppPaths`·오류 타입 | Linux에서 가능(Windows·macOS 테스트는 CI가 실행) |
| 3 | [M1-02 rf-platform과 rf-vfs](2026-10-22-m1-02-vfs-local.md) | OS 기능(자연 정렬, 휴지통, 파일 ID, 감시), `Provider` 트레이트, `LocalProvider`, 정렬·필터, 10만 항목 벤치마크 | 과제 1~14는 Linux, 15~18은 실기 |
| 4 | [M1-03 rf-jobs](2026-10-22-m1-03-jobs-engine.md) | 복사·이동·삭제 작업 엔진, 충돌 처리, `.rfpart` 원자적 쓰기, 저널·복구, 되돌리기 기록, 결함 주입 테스트 | 과제 1~19는 Linux(OS별 테스트는 CI), 20~21은 실기 |
| 5 | [M1-04 rf-ledger](2026-10-22-m1-04-change-ledger.md) | 변경 원장(선기록, 역순 복원), HKCU 레지스트리·macOS 기본값 백엔드, 클린 삭제 코어 | Linux에서 가능(OS 백엔드는 CI가 실행) |
| 6 | [M1-05 데모 UI 셸](2026-10-29-m1-05-demo-ui-shell.md) | `rf-ui`(Slint) 듀얼 패널 데모 앱: 탐색, 탭, 폴더 트리, 새 폴더·이름 바꾸기·휴지통·복사·이동(PICK)·충돌·되돌리기, 설정, OS별 데모 산출물 | Linux에서 가능(Xvfb 스크린샷, Windows·macOS 실행 파일은 CI가 만듦) |

- M1-04는 M1-01 뒤라면 언제 실행해도 된다. M1-02와 M1-03은 이 순서대로 실행한다.
- M1-01~04는 UI와 무관한 코어라서 M0와 병행할 수 있다. 다만 최소 Rust 버전은 M0의 결정 D2를 따른다(아래 참고).

## 아직 없는 계획

M0 결정 게이트를 통과한 뒤에 작성한다.

- M1 UI 셸: Slint 듀얼 패널, 트리, 탭, 가상화 목록, PICK 모달, 테마, 작업 완료 OS 알림(XFER-14), 트레이·메뉴 막대 진행 표시(XFER-15). 데모 범위는 M1-05가 먼저 구현했다.
- M1 Windows 플랫폼 통합: 셸 메뉴, OLE 드래그 앤 드롭, 썸네일, 탐색기 대체 런처, 설치 프로그램
- M0 결정 D1이 Qt 6이면 Qt 스파이크 계획을 먼저 쓴다.

## 계획 작성 중 확인된 사실

M0 결정에 영향을 주는 사항이다.

1. **최소 Rust 버전.** 2026-10-22 기준 Slint 1.18.1은 Rust 1.92, OpenDAL 0.59는 1.91을 요구한다. 워크스페이스 `rust-version`을 1.85로 두면 `cargo add`가 오래된 버전을 고른다. 예를 들어 드래그 앤 드롭이 없는 1.17 이전의 Slint가 선택된다. M0 과제 22에서 D2로 확정하고, M1 계획의 버전 관련 기대 출력은 D2 기준으로 읽는다.
2. **Slint의 OS 드래그.** Slint 1.18.1의 winit 백엔드는 OS로 파일을 끌어내는 기능을 구현하지 않았다. M0 게이트 3은 OS 계층(OLE, NSDragging)에서 직접 구현해 검증한다(PRD R1).
3. **OpenDAL SFTP.** OpenDAL의 SFTP는 Unix 전용 크레이트에 의존해서 Windows에서 빌드되지 않는다. FTP 설정에는 능동/수동 모드와 파일명 인코딩 옵션이 없다. 그래서 SFTP·FTP는 직접 구현(russh-sftp 등)할 가능성이 높고, M0 과제 17에서 확인한다(PRD R9).

## 실행 방법

각 계획은 Claude Code에서 `superpowers:subagent-driven-development`(과제마다 새 에이전트가 구현하고 리뷰어가 검토) 또는 `superpowers:executing-plans`(한 세션에서 순서대로 구현)로 실행한다. 단계는 체크박스(`- [ ]`)로 진행 상황을 남긴다.
