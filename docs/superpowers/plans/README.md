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
4. **Slint의 앱 안 드래그(M1-05 Task 9에서 확인).** Slint 1.18.1의 `DragArea`/`DropArea`로 패널·탭·트리 사이 끌어 놓기를 만들 수 있다. `data-transfer`는 `.slint`에서 열어 볼 수 없고 Rust에서 `slint::DataTransfer`로 만들고 읽는다. 앱 안에서만 쓰는 값(`set_user_data`)만 담으면 OS 드래그를 거치지 않는 창 안 드래그가 되고, 커서는 Slint가 동작에 맞게(이동 기본 화살표, 복사 +, 거부 금지 표시) 바꾼다. 다음은 지원하지 않거나 다르게 동작해 데모가 범위를 줄이거나 우회했다. (1) 끌기 시작을 알리는 콜백이 없다(`dragging` 속성과 `drag-finished`뿐). 그래서 행을 누를 때 옮길 항목을 정해 두고 그 id를 페이로드에 담는다. (2) 보조키 규칙은 Slint가 정한다: 없음은 허용한 첫 동작(이동), Ctrl은 복사, Shift는 이동, Ctrl+Shift는 링크(허용하지 않으면 이동). `DropEvent`에 보조키가 없어 앱이 바꿀 수 없다. macOS의 winit 백엔드는 Cmd를 `control`로 바꿔 주므로 Cmd+끌기가 복사이고 Option은 아무 효과가 없다. PRD DND-03의 Finder 규칙(Option=복사, Cmd=이동)과 다르며 Windows의 Alt(바로가기)도 없다. 끄는 도중 보조키를 누르면 `can-drop`이 다시 불린다. (3) 겹친 `DropArea` 중 안쪽이 거부하면 이벤트가 바깥으로 넘어가 바깥이 받는다(i-slint-core의 이벤트 전달 코드). 그래서 목록 전체를 하나의 `DropArea`로 두고 포인터 위치로 행을 계산한다(폴더 행이 거부해도 패널 폴더로 떨어지지 않게). (4) 머무르면 열기(스프링 로드)·가장자리 자동 스크롤(DND-08)은 내장되지 않았다. (5) 끄는 그림(`drag-image`)은 이미지만 받는다. 커서 옆 표시(DND-03)는 각 `DropArea`가 받는 포인터 위치를 창 좌표로 옮겨 창 위 겹침 요소로 그리면 만들 수 있지만, 데모는 단순하게 놓을 목록 아래쪽에 "항목 N개 이동 → 폴더" 문구를 띄우기로 했다(Slint의 한계가 아니라 데모의 선택). 탭과 트리 위에서는 accent 테두리와 커서 모양만 바뀐다. (6) 테스트 백엔드에서 포인터 이벤트로 끌어 놓기를 흉내 낼 수 있다. 입력기 조합(preedit)은 공개 API로 보낼 수 없어 `i-slint-core`의 내부 이벤트로 보낸다.

## 실행 방법

각 계획은 Claude Code에서 `superpowers:subagent-driven-development`(과제마다 새 에이전트가 구현하고 리뷰어가 검토) 또는 `superpowers:executing-plans`(한 세션에서 순서대로 구현)로 실행한다. 단계는 체크박스(`- [ ]`)로 진행 상황을 남긴다.
