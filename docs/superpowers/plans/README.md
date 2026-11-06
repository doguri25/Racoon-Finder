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
| 7 | [M1-06 데모 2판](2026-10-29-m1-06-demo-2.md) **(완료, 0.2.0)** | OS별 단축키 배정표, 레이아웃 전환·분할선, 탭·주소창 고급, 보기 모드·열·폴더별 보기, 선택 도구, 빠른 보기·미리보기 패널, OS 호환 클립보드·경로 복사, 속성·체크섬, 연결 프로그램·터미널, 완료 보고·OS 알림·트레이, 코어 후속 정리, 0.2.0 | Linux에서 구현(OS 코드는 CI가 검증, 대화형 세션이 필요한 검사는 손으로) |
| 8 | [M1-07 데모 3판](2026-11-02-m1-07-demo-3.md) **(완료, 0.3.0)** | 다른 앱과 주고받는 끌어 놓기(Windows OLE, macOS NSDragging), 놓을 때의 동작(OS별 보조키·설정·묻기), 머무르면 열기·자동 스크롤·이동 되돌리기 알림, Windows 셸 메뉴, macOS 공유, 0.3.0 | Linux에서 구현(OS 코드는 CI가 검증, 실제 끌기와 메뉴는 손으로) |
| 9 | [M1-08 데모 4판](2026-11-05-m1-08-demo-4.md) **(완료, 0.4.0)** | OS 썸네일(VIEW-09), 즐겨찾기 편집·끌어 놓아 추가·최근 폴더, 링크 만들기(바로 가기·별칭·심볼릭 링크·정션), 마우스 4·5번·트랙패드·길게 누르기·체크박스 선택, 되돌리기 기록·상태 표시줄, Windows 설치 프로그램(사용자 단위, AUMID, 클린 삭제), macOS 디스크 이미지와 완전 삭제, 설정 버전, 0.4.0 | Linux에서 구현(OS 코드는 CI가 검증, 썸네일·링크·설치는 손으로) |

- M1-04는 M1-01 뒤라면 언제 실행해도 된다. M1-02와 M1-03은 이 순서대로 실행한다.
- M1-01~04는 UI와 무관한 코어라서 M0와 병행할 수 있다. 다만 최소 Rust 버전은 M0의 결정 D2를 따른다(아래 참고).

## 아직 없는 계획

M0 결정 게이트를 통과한 뒤에 작성한다.

- M1 UI 셸: Slint 듀얼 패널, 트리, 탭, 가상화 목록, PICK 모달, 테마, 작업 완료 OS 알림(XFER-14), 트레이·메뉴 막대 진행 표시(XFER-15). 데모 범위는 M1-05가 먼저 구현했고 M1-06이 넓혔다.
- M1 Windows 플랫폼 통합: 탐색기 대체 런처(WINX), 상주 모드, 보조 프로세스 격리(VIEW-08), 서명·공증, winget·Homebrew(INST-09)
- M0 결정 D1이 Qt 6이면 Qt 스파이크 계획을 먼저 쓴다.

## 계획 작성 중 확인된 사실

M0 결정에 영향을 주는 사항이다.

1. **최소 Rust 버전.** 2026-10-22 기준 Slint 1.18.1은 Rust 1.92, OpenDAL 0.59는 1.91을 요구한다. 워크스페이스 `rust-version`을 1.85로 두면 `cargo add`가 오래된 버전을 고른다. 예를 들어 드래그 앤 드롭이 없는 1.17 이전의 Slint가 선택된다. M0 과제 22에서 D2로 확정하고, M1 계획의 버전 관련 기대 출력은 D2 기준으로 읽는다.
2. **Slint의 OS 드래그.** Slint 1.18.1의 winit 백엔드는 OS로 파일을 끌어내는 기능을 구현하지 않았다. M0 게이트 3은 OS 계층(OLE, NSDragging)에서 직접 구현해 검증한다(PRD R1).
3. **OpenDAL SFTP.** OpenDAL의 SFTP는 Unix 전용 크레이트에 의존해서 Windows에서 빌드되지 않는다. FTP 설정에는 능동/수동 모드와 파일명 인코딩 옵션이 없다. 그래서 SFTP·FTP는 직접 구현(russh-sftp 등)할 가능성이 높고, M0 과제 17에서 확인한다(PRD R9).
4. **Slint의 앱 안 드래그(M1-05 Task 9에서 확인).** Slint 1.18.1의 `DragArea`/`DropArea`로 패널·탭·트리 사이 끌어 놓기를 만들 수 있다. `data-transfer`는 `.slint`에서 열어 볼 수 없고 Rust에서 `slint::DataTransfer`로 만들고 읽는다. 앱 안에서만 쓰는 값(`set_user_data`)만 담으면 OS 드래그를 거치지 않는 창 안 드래그가 되고, 커서는 Slint가 동작에 맞게(이동 기본 화살표, 복사 +, 거부 금지 표시) 바꾼다. 다음은 지원하지 않거나 다르게 동작해 데모가 범위를 줄이거나 우회했다. (1) 끌기 시작을 알리는 콜백이 없다(`dragging` 속성과 `drag-finished`뿐). 그래서 행을 누를 때 옮길 항목을 정해 두고 그 id를 페이로드에 담는다. (2) 보조키 규칙은 Slint가 정한다: 없음은 허용한 첫 동작(이동), Ctrl은 복사, Shift는 이동, Ctrl+Shift는 링크(허용하지 않으면 이동). `DropEvent`에 보조키가 없어 앱이 바꿀 수 없다. macOS의 winit 백엔드는 Cmd를 `control`로 바꿔 주므로 Cmd+끌기가 복사이고 Option은 아무 효과가 없다. PRD DND-03의 Finder 규칙(Option=복사, Cmd=이동)과 다르며 Windows의 Alt(바로가기)도 없다. 끄는 도중 보조키를 누르면 `can-drop`이 다시 불린다. (3) 겹친 `DropArea` 중 안쪽이 거부하면 이벤트가 바깥으로 넘어가 바깥이 받는다(i-slint-core의 이벤트 전달 코드). 그래서 목록 전체를 하나의 `DropArea`로 두고 포인터 위치로 행을 계산한다(폴더 행이 거부해도 패널 폴더로 떨어지지 않게). (4) 머무르면 열기(스프링 로드)·가장자리 자동 스크롤(DND-08)은 내장되지 않았다. (5) 끄는 그림(`drag-image`)은 이미지만 받는다. 커서 옆 표시(DND-03)는 각 `DropArea`가 받는 포인터 위치를 창 좌표로 옮겨 창 위 겹침 요소로 그리면 만들 수 있지만, 데모는 단순하게 놓을 목록 아래쪽에 "항목 N개 이동 → 폴더" 문구를 띄우기로 했다(Slint의 한계가 아니라 데모의 선택). 탭과 트리 위에서는 accent 테두리와 커서 모양만 바뀐다. (6) 테스트 백엔드에서 포인터 이벤트로 끌어 놓기를 흉내 낼 수 있다. 입력기 조합(preedit)은 공개 API로 보낼 수 없어 `i-slint-core`의 내부 이벤트로 보낸다.
5. **Slint·winit과 OS 끌어 놓기(M1-07에서 확인).** 4의 (2)를 고치고 다음을 더한다.
   - **놓을 때의 동작은 앱이 정한다.** `drop-check`가 돌려주는 `DragAction`이 놓기의 동작이 되고, 커서 모양도 그 답을 따른다. 앱은 Slint의 보조키 규칙 대신 winit의 `ModifiersChanged`로 OS별 규칙(macOS ⌥ 복사, ⌘ 이동)을 쓴다.
   - **winit은 포커스를 잃으면 보조키를 비운다.** 다른 앱이 끄는 동안 창에는 포커스가 없으므로, 그 끌기에는 OS가 끌기와 함께 알리는 보조키를 쓴다.
   - **macOS의 받는 곳.** winit 0.30은 창 대리자(window delegate)가 창 단위로 끌기를 받고 놓인 파일만 알린다(위치·보조키 없음). 앱은 창 내용 위에 끌기만 받는 투명한 하위 뷰를 얹는다. 이 뷰가 실제 Mac에서 끌기를 받는지는 아직 확인하지 못했다(keyboard.md "끌어 놓기"의 18번).
   - **Windows의 받는 곳.** winit이 등록한 드롭 대상을 `RevokeDragDrop` 뒤 `RegisterDragDrop`으로 앱의 것으로 바꾼다. winit의 `FileDropHandler`는 `QueryInterface`가 `unimplemented!()`라서 감싸 쓸 수 없다.
   - **Windows의 모달 루프.** `SHDoDragDrop`(끌어내기)과 `TrackPopupMenuEx`(셸 메뉴)가 도는 동안 winit은 이벤트를 모아 두기만 해서 창이 다시 그려지지 않는다(winit 0.30 소스로 확인. 실제 PC에서는 keyboard.md "끌어 놓기"의 31번으로 확인).
   - **Slint의 끌기를 놓기 없이 끝내기.** `PointerExited`를 보내면 끌기가 `drag-finished`의 `none`으로 끝나고 놓이지 않는다. 창 밖으로 나간 끌기를 OS의 끌기로 넘길 때 쓴다.
6. **썸네일, 제스처, 설치 도구(M1-08에서 확인).**
   - **macOS QuickLook의 스레드.** `QLThumbnailGenerator`는 주 스레드가 필요 없다. 헤더와 `objc2-quick-look-thumbnailing`의 바인딩 어디에도 `QLThumbnailGenerator`·`QLThumbnailGenerationRequest`·`QLThumbnailRepresentation`에 주 스레드 표시(`MainThreadMarker`)가 없고, 생성기는 완료 핸들러를 자기 큐에서 부른다. 그래서 작업 스레드에서 `generateBestRepresentationForRequest:completionHandler:`를 부르고 채널로 답을 기다리면 된다(30초까지). 실제 Mac에서 번들 밖의 테스트 바이너리로 도는지는 아직 확인하지 못했다(keyboard.md "썸네일 공급자" 2번).
   - **Windows `IShellItemImageFactory`의 "썸네일 없음" 코드.** 셸이 "이 파일은 썸네일을 줄 수 없다"고 답하는 코드는 `E_FAIL`, `0x8004B200`(`WTS_E_FAILEDEXTRACTION`), `WINCODEC_ERR_COMPONENTNOTFOUND`, `WINCODEC_ERR_UNSUPPORTEDPIXELFORMAT`, `WINCODEC_ERR_UNKNOWNIMAGEFORMAT`, `WINCODEC_ERR_BADHEADER`, `WINCODEC_ERR_BADIMAGE`뿐이다. 이것만 `None`(종류별 아이콘으로 남음)이고 나머지는 모두 `Failed`(다시 시도)다. 파일 내용에서 오는 코드는 파일이 바뀔 때까지 같고(캐시 키에 수정 시각과 크기가 있다), 전송·상태에서 오는 코드(`WTS_E_NOSTORAGEPROVIDERTHUMBNAILHANDLER`, `WTS_E_EXTRACTIONTIMEDOUT`, `WINCODEC_ERR_STREAMREAD`, 공유 위반 등)는 다시 시도해야 하기 때문이다. 셸이 돌려주는 비트맵은 미리 곱한 알파이고 요청보다 클 수 있어(`SIIGBF_BIGGERSIZEOK`) 앱이 줄인다. COM은 STA여야 하므로 작업 스레드는 MTA가 아닌 평범한 스레드여야 한다.
   - **Inno Setup의 AUMID.** `[Icons]` 항목의 `AppUserModelID:` 매개변수가 시작 메뉴 바로 가기에 AUMID를 적는다. 앱의 `identity::AUMID`와 같은 값(`io.github.doguri25.RacoonFinder`)을 쓰면 설치형 앱의 알림이 "Racoon Finder"라는 이름과 아이콘으로 뜬다. 압축만 푼 포터블은 바로 가기가 없어 AUMID가 없고, 알림은 "Windows PowerShell"로 뜬다. 시작 메뉴 바로 가기가 없으면 AUMID를 단 알림이 뜨지 않을 수 있다.
   - **winit의 제스처.** winit 0.30.13의 `PanGesture`는 iOS에서만 온다(문서: "Only available on iOS"). 데스크톱에서는 트랙패드 두 손가락 스와이프로 뒤로·앞으로를 할 수 없다. macOS는 두 손가락 가로 스크롤을 `MouseWheel`로 주고 세 손가락 스와이프(`swipeWithEvent:`)는 내놓지 않으며, Windows 정밀 터치패드의 스와이프는 가로 휠로 온다. `PinchGesture`는 macOS에서 온다. 마우스 4·5번 버튼은 `MouseButton::Back`·`Forward`로 온다(Windows XBUTTON1·2, macOS 다른 버튼 3·4, X11 8·9).
   - **Inno Setup 설치와 `ISCC` 찾기.** 스크립트는 Inno Setup 6.3 이상이 필요하다(6.3이 `x64compatible`과 공식 한국어 번역 `Languages\Korean.isl`을 더했다). 러너(windows-latest) 이미지의 chocolatey 기록(6.7.1)에는 컴파일러(`ISCC.exe`)가 디스크에 없었고 6.4.3 패키지도 없다. 그래서 CI는 이미 설치된 6.3 이상 6.x의 `ISCC.exe`를 찾고(`PATH`, 프로그램 폴더, chocolatey 폴더), 없으면 jrsoftware의 내려받기 페이지(`https://jrsoftware.org/download.php/is.exe`)에서 현재 안정판 설치 프로그램을 받아 사용자 단위로 알려진 폴더에 설치한 뒤 버전을 확인한다(7.x는 거절).

## 실행 방법

각 계획은 Claude Code에서 `superpowers:subagent-driven-development`(과제마다 새 에이전트가 구현하고 리뷰어가 검토) 또는 `superpowers:executing-plans`(한 세션에서 순서대로 구현)로 실행한다. 단계는 체크박스(`- [ ]`)로 진행 상황을 남긴다.
