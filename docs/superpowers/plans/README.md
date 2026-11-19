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
| 10 | [M1-09 데모 5판](2026-11-13-m1-09-demo-5.md) **(완료, 0.5.0)** | M1 Alpha의 남은 빈칸: 전송의 남은 선택(다시 시도, 정책 두 가지, 현재 파일·속도 그래프·순서 바꾸기), 새 파일·Tab 이름 바꾸기·방문 기록, 패널별 트리·툴바 숨김·창 위치, 썸네일 보기·타일·열, 휴지통과 내 PC, 테마 토큰·고대비·밀도·글자 크기, 한국어·영어(Fluent), 0.5.0 | Linux에서 구현(OS 코드는 CI가 검증, 휴지통·고대비·언어는 손으로) |

- M1-04는 M1-01 뒤라면 언제 실행해도 된다. M1-02와 M1-03은 이 순서대로 실행한다.
- M1-01~04는 UI와 무관한 코어라서 M0와 병행할 수 있다. 다만 최소 Rust 버전은 M0의 결정 D2를 따른다(아래 참고).

## 아직 없는 계획

M0 결정 게이트를 통과한 뒤에 작성한다.

- M1 UI 셸: Slint 듀얼 패널, 트리, 탭, 가상화 목록, PICK 모달, 테마, 작업 완료 OS 알림(XFER-14), 트레이·메뉴 막대 진행 표시(XFER-15). 데모 범위는 M1-05가 먼저 구현했고 M1-06이 넓혔다.
- M1 Windows 플랫폼 통합: 탐색기 대체 런처(WINX), 상주 모드, 보조 프로세스 격리(VIEW-08), 권한 상승 보조 프로세스(OPS-16, Windows UAC),
  깨끗한 VM에서의 설치 검증(INST-07), 압축 파일 연결(INST-01의 나머지), 서명·공증, winget·Homebrew(INST-09)
- M1 창 여러 개(LAY-07의 나머지)와 창 사이 끌어 놓기(DND-01). 같은 계획에서 할 것: 작업 패널의 별도 창을 주 창에 딸린 창으로
  만들고(지금은 따로 선 창이라 뜰 때 키보드를 가져가고 겹치면 주 창 뒤로 간다) 그 위치·크기를 저장하기, **앱이 그리는 메뉴**(지금의
  메뉴는 Windows·macOS에서 OS 메뉴, Linux에서 Slint fluent 스타일의 창 안 메뉴다. Linux 메뉴는 키보드가 놓인 항목을 색으로만 알려
  고대비에서 1.2:1쯤이고, 어느 OS에서나 메뉴는 앱의 글자 크기를 따르지 않는다: Slint 1.18.1의 `MenuItem`에는 글자 크기도 화면 읽기용
  설명도 없고 팝업이 창의 `default-font-size`를 물려받지 않는다), 창 아래에 붙은 작업 패널의 높이 상한(900×560 넉넉하게 + 130%에서
  대기 작업과 머리 줄이 보이면 패널마다 두 줄쯤, 최근 완료를 펼치면 보통·100%에서도 거의 남지 않는다).
- M1-09가 일부러 넘긴 나머지: 재시작 없이 언어 바꾸기(I18N-01의 P1), 날짜·숫자 현지화(I18N-03, M2), 테마 가져오기·내보내기와 프리셋
  6종(THEME-02·05, M3), 네트워크 위치(NAV-16의 일부), 즐겨찾기 그룹(NAV-05의 일부), EXIF·ID3 등 추가 열(NAV-07의 나머지), 그룹 헤더
  (NAV-09, P1), 빠른 필터의 와일드카드·정규식(FIND-01, M2), 압축(ARC), 하드 링크·속성 바꾸기(OPS-08·09, P1), 휴지통으로 보내기와 영구
  삭제의 다시 시도(XFER-09의 나머지: `rf_jobs::retry_spec`은 이미 받는다. 다른 볼륨 걸러 내기를 먼저 넣는다).
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
   - **Inno Setup 설치와 `ISCC` 찾기.** 스크립트는 Inno Setup 6.3 이상이 필요하다(6.3이 `x64compatible`과 공식 한국어 번역 `Languages\Korean.isl`을 더했다). 러너(windows-latest) 이미지의 chocolatey 기록(6.7.1)에는 컴파일러(`ISCC.exe`)가 디스크에 없었고 6.4.3 패키지도 없다. 그래서 CI는 이미 설치된 6.3 이상 6.x의 `ISCC.exe`를 찾고(`PATH`, 프로그램 폴더, chocolatey 폴더), 없으면 `choco install innosetup --version=6.7.1 --force`로 다시 설치한 뒤 다시 찾는다(7.x는 거절).
7. **문구, 휴지통, 고대비, 창 자리(M1-09에서 확인).**
   - **Fluent와 Slint 전역 `Tr`.** 문구의 원본은 `crates/rf-ui/i18n/{ko,en}/<영역>.ftl`(Project Fluent, `fluent-bundle`)이고 `include_str!`로 빌드에 들어간다. 키는 `영역-뜻`, 수와 이름은 Fluent 인자, 영어의 단·복수는 선택자, OS마다 다른 이름(Recycle Bin/Trash, Exit/Quit)은 모든 메시지에 주는 `$os` 인자로 고른다. 화면(`.slint`)의 문구는 Slint 전역 `Tr`(`ui/tr.slint`)의 값 없는 `in property <string>`이고 속성 하나가 `ui.ftl`의 키 하나다. build.rs가 `tr.slint`를 엄격히 읽어 속성마다 setter 표를 만들므로 채우기에서 빠지는 속성이 없다. **Slint의 전역은 창 인스턴스마다 따로다**(트레이 아이콘의 `SystemTrayIcon`도 자기 전역을 가진다). 그래서 창을 만드는 곳은 `ui_bridge`의 네 maker(`new_main_window`, `new_notice_window`, `new_job_window`, `new_job_tray`)뿐이고, 만들 때 모든 속성을 채우며, 다른 곳에서 창을 만들지 않는다는 것을 테스트가 지킨다. 인자가 있는 화면 문구는 **방법 (a)**로 통일했다: Rust가 완성한 문자열을 모델 필드나 속성으로 준다(`screen.ftl`). Slint 1.18.1에 `string.replace-all`은 있지만 틀 문자열로는 복수형도 한국어 조사도 고를 수 없다. 언어는 시작할 때 창을 띄우기 전에 한 번 정한다(재시작 없이 바꾸기는 P1). 테스트는 늘 한국어로 돌고, 영어는 `Localizer` 인스턴스와 `ui_bridge::speak_in`으로 시험한다.
   - **휴지통 열거의 OS별 한계.** 휴지통은 변경 알림이 없어 감시할 수 없고, Linux·macOS는 비운 이름을 곧바로 다시 쓰므로 자리(이름)가 신원이 아니다. 그래서 목록이 읽은 항목의 **신원 도장**(`TrashStamp`: 볼륨·파일 ID, Unix의 ctime, 크기, 기록 파일의 해시)이 지금도 맞을 때만 복원·영구 삭제하고 아니면 `TrashItemChanged`로 거절한다. ext4는 커널 6.13 이전에 지운 뒤 같은 inode를 다시 주고 ctime이 거칠어 크기를 도장에 넣었다. 휴지통으로 보낸 작업의 되돌리기 기록도 같은 도장을 선택 필드로 가진다(전의 기록은 그대로 읽힌다). 비우기는 모든 OS에서 확인 창이 센 목록을 항목마다 지우는 것이다(`SHEmptyRecycleBinW`도 앱이 비우기에는 쓰지 않는다). **Windows:** 고정 드라이브마다 `X:\$Recycle.Bin\<사용자 SID>`의 `$I…`(원래 경로·삭제 시각·크기, v1은 정확히 544바이트, v2는 길이가 맞아야 한다)와 짝 `$R…`을 읽고, 영구 삭제는 항목마다 `$R`과 `$I`를 지운다. 셸에 따로 알리지 않아 탐색기의 휴지통 아이콘이 따라오는지는 손으로 확인한다. **macOS:** `~/.Trash`와 `/Volumes/*/.Trashes/<uid>`를 읽는다. 원래 위치는 Finder가 공개하지 않아 알 수 없고("되돌려 놓기"의 기록은 비공개), 삭제 시각은 항목의 ctime으로 대신하며, `~/.Trash` 목록은 전체 디스크 접근(TCC) 권한이 있어야 읽힌다. 지우기는 `NSFileManager`. APFS는 UTF-8이 아닌 이름을 거절한다(EILSEQ). **Linux:** freedesktop 규격의 홈 휴지통(`$XDG_DATA_HOME/Trash`)만 쓰고 볼륨의 `.Trash-<uid>`는 읽지 않는다. `.trashinfo`의 `Path=`는 퍼센트 인코딩이고 상대 경로는 그 휴지통이 있는 곳에 대해 읽는다. 임시 폴더(`/tmp`, `/var/tmp`, `TMPDIR`) 안의 항목은 휴지통이 없는 곳으로 본다(테스트가 실제 홈 휴지통을 채우지 않게).
   - **OS의 고대비 읽기.** Windows는 `SystemParametersInfoW(SPI_GETHIGHCONTRAST)`의 `HCF_HIGHCONTRASTON`, macOS는 `NSWorkspace.accessibilityDisplayShouldIncreaseContrast`, Linux는 `GTK_THEME`가 고대비 테마를 가리킬 때뿐이다(GNOME의 고대비 스위치는 dconf라 D-Bus나 파일을 읽어야 해서 보지 않는다). Windows·macOS는 고대비만 켜고 끌 때 창에 알리지 않을 때가 있어 창이 앞으로 올 때 다시 읽는다. 시스템 설정 따르기에서는 Slint의 `Palette.color-scheme`을 `unknown`으로 두어야 OS의 밝게·어둡게를 계속 받는데, Windows는 대비 테마에서 밝게라고 알려 Slint의 스크롤바가 밝게 그려질 수 있다. `Palette.color-scheme`은 Slint fluent 스타일의 전역일 뿐 winit·muda에 닿지 않으므로(앱은 winit의 `set_theme`도 부르지 않는다), Windows·macOS의 OS 메뉴는 앱이 고른 테마와 상관없이 OS의 모양(밝게·어둡게, 대비 테마)으로 그려진다. Linux의 Slint 메뉴는 앱의 테마를 따른다.
   - **Slint의 메뉴.** `ContextMenuArea`의 메뉴(이 앱의 보기·선택·오른쪽 클릭 메뉴)는 Windows·macOS에서 winit 백엔드가 muda로 OS 메뉴를 띄우고(i-slint-backend-winit 1.18.1의 `show_native_popup_menu`), Linux에서는 Slint가 fluent 스타일로 창 안에 그린다. 그 강조색(`FluentPalette`)은 컴파일 때 정해져 앱이 바꿀 수 없다.
   - **창 자리 복원의 플랫폼 차이.** winit의 `resumed`에서 화면 목록에 맞춰 순수 함수 `placement`가 자리를 정한다(winit은 화면 전체만 알려 주므로 테두리 16×40과 작업 표시줄 48 논리 픽셀을 어림한다). Slint는 처음 보일 때 창을 선호 크기로 다시 맞추고 winit은 크기를 요청받은 창의 최대화를 풀므로, 크기는 창을 보이기 전에 `set_size`로 주고 그다음 최대화한다. **X11**의 창 관리자는 보이기 전의 최대화를 받지 않아 처음 보인 뒤 한 번 더 최대화하고, 그 사이 오는 "최대화 아님" 알림은 설정을 지우지 않는다. **Windows**는 최소화한 창을 (-32000, -32000)에 두므로 최소화·최대화 동안의 자리는 저장하지 않는다. 위치와 크기는 `Moved`·`Resized` 알림에서 기억해 두고 저장할 때 묻지 않는다(X11의 `outer_position`은 왕복이다). **Wayland**는 앱에 창의 위치를 알려 주지도 정하게 하지도 않고 주 화면도 없어, 크기와 최대화만 쓰고 크기는 winit이 처음 알려 주는 화면에 맞춘다.
   - **스모크 테스트는 글자로 잰 크기를 고정 수와 견주지 않는다.** 러너마다 글꼴이 달라(이 기계의 WenQuanYi와 Ubuntu·macOS 러너의 글꼴은 같은 줄을 830 px과 804 px로 잰다) 고정 수에 맞춘 단언이 CI에서 깨졌다. 테스트는 잰 너비로 규칙을 단언하고, 넘침 같은 경우는 아주 짧거나 긴 내용으로 반드시 일어나게 만든다. 테스트 백엔드의 글자는 실제 글꼴보다 좁으므로 들어가는지는 Xvfb 스크린샷이 본다.
   - **그 밖에.** Slint는 경로(`Path`)의 선 굵기를 viewbox와 상관없이 논리 px로 그려, 큰 아이콘은 선 굵기를 크기에 맞춰 키워야 한다. rustix 1.1.6의 `llistxattr`·`lgetxattr`는 길이 0인 버퍼에 크기를 돌려주지 않으므로 `*_query`를 쓴다. `RUST_BACKTRACE=1`에서 한 프로세스의 첫 백트레이스는 테스트 바이너리의 디버그 정보를 디스크에서 읽어(찬 캐시에서 10초 넘게) 일부러 패닉하고 시간 한도를 기다리는 테스트는 `resume_unwind`로 풀어야 한다.

## 실행 방법

각 계획은 Claude Code에서 `superpowers:subagent-driven-development`(과제마다 새 에이전트가 구현하고 리뷰어가 검토) 또는 `superpowers:executing-plans`(한 세션에서 순서대로 구현)로 실행한다. 단계는 체크박스(`- [ ]`)로 진행 상황을 남긴다.
