# M1-05 데모 UI 셸 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** M1 코어(rf-core, rf-platform, rf-vfs, rf-jobs) 위에 사용자가 자기 PC(Windows 11, macOS 15)에서 직접 실행해 볼 수 있는 데모 앱을 만든다. 듀얼 패널로 실제 폴더를 탐색하고, 새 폴더·이름 바꾸기·휴지통 삭제·복사·이동(PICK 모달)·충돌 처리·되돌리기를 실제 파일에 수행하며, CI가 OS별 실행 파일을 내려받을 수 있는 산출물로 만든다.

**Architecture:** 새 라이브러리 크레이트 `crates/rf-ui`가 Slint 화면(`ui/*.slint`)과 뷰모델을 담고, 새 실행 파일 크레이트 `apps/racoon-finder`는 `rf_ui::run`만 부른다. 화면 상태는 UI 스레드의 Rust 구조체(`AppState`)가 갖고, 파일 시스템을 건드리는 모든 호출은 작업 스레드에서 돌린 뒤 `slint::invoke_from_event_loop`로 결과를 UI 스레드에 넘긴다. 상태 전이(탐색 기록, 선택, 정렬, 목록 갱신, PICK 유효성, 문구)는 Slint와 무관한 순수 Rust 모듈이라 단위 테스트로 검증하고, Slint 쪽은 `i-slint-backend-testing`의 헤드리스 테스트와 Xvfb 스크린샷으로 확인한다. 파일을 바꾸는 작업은 전부 `rf_jobs::JobEngine`을 거쳐 되돌리기·충돌 처리·저널 복구를 그대로 얻는다.

**Tech Stack:** Rust stable(edition 2024), Slint 1.18.1(GPLv3로 사용, 기본 기능: winit 백엔드, femtovg·software 렌더러, 접근성), i-slint-backend-testing 1.18.1(개발 의존성), jiff(로컬 시간 표시), opener(OS 기본 앱으로 열기), toml·serde(설정), tempfile(테스트).

**Spec:** docs/PRD.md(5.1 LAY, 5.2 NAV, 5.3 OPS, 5.4 DND, 5.5 PICK, 5.6 XFER, 5.20 THEME, 5.22 KEY). 디자인 기준은 목업 캔버스 "Racoon Finder 데모 목업"(메인 창, PICK 모달, 이름 충돌 창)이며, 그 팔레트·문구·배치를 이 문서의 "화면 사양"에 옮겨 적었다.

## 이 계획의 위치

`docs/superpowers/plans/README.md`는 "M1 UI 셸"을 M0 결정 게이트 뒤에 쓰기로 했다. 사용자가 실물 데모를 먼저 요청했으므로 이 계획은 M0 게이트 1(Slint)을 통과했다고 가정한 데모 범위의 UI 셸이다. M0 결과로 Qt가 선택되면 `rf-ui`만 바뀌고 이 계획의 순수 모듈(Task 2, 3, 5, 7의 상태 코드)은 그대로 쓴다. 정식 M1 UI 계획은 이 데모를 출발점으로 범위를 넓힌다(아래 "다루지 않는 항목").

## Global Constraints

- M1-01 계획의 Global Constraints를 따른다(edition 2024, `GPL-3.0-or-later`, 매니페스트 상속 블록과 `[lints] workspace = true`, `cargo add`로만 의존성 추가, `Cargo.lock` 커밋, 영어 conventional commits, 표준 검사·교차 검사, `unsafe` 금지). 다음은 예외이거나 추가 규칙이다.
- **최소 Rust 버전:** 루트 `Cargo.toml`의 `workspace.package.rust-version`을 `1.92`로 올린다(Task 1). Slint 1.18.1이 1.92를 요구하고, resolver 3이 1.85 기준으로 오래된 Slint를 고르는 문제(plans/README.md "계획 작성 중 확인된 사실" 1)를 없앤다. M0 결정 D2의 예상값이다.
- **고쳐도 되는 루트 파일:** 이 계획은 루트 `Cargo.toml`(rust-version 한 줄), `.github/workflows/ci.yml`(Linux 패키지 설치 단계, 시간 제한), 새 `.github/workflows/demo.yml`, `deny.toml`(필요할 때 `allow`에 한 줄씩 추가, 커밋 메시지에 GPL 호환 근거), `README.md`(데모 실행 안내 절)를 고친다. `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`은 고치지 않는다.
- **제품 이름:** `crates/`의 `.rs`에는 여전히 제품 이름 철자를 쓰지 않는다(`product_name_appears_only_in_identity_module`). 창 제목 등은 Rust에서 `rf_core::identity::DISPLAY_NAME`을 Slint 속성으로 넘긴다. `.slint` 파일에도 제품 이름을 쓰지 않는다(같은 이유로 이름이 바뀔 때 한 곳만 고치도록). `apps/racoon-finder`의 크레이트 이름과 실행 파일 이름 `racoon-finder`는 PRD 8.2의 이름이라 예외다.
- **UI 스레드 규칙:** UI 스레드에서는 `Provider`의 어떤 메서드도, `rf_platform`의 `disk_space`·`volume_info`·`trash_supported`·`known_folder`도, `JobEngine::shutdown`·`JobReport::write_log`도 부르지 않는다. 이 호출은 작업 스레드(`std::thread`)에서 돌리고 결과를 `slint::invoke_from_event_loop`(또는 `Weak::upgrade_in_event_loop`)로 넘긴다. `JobEngine`의 나머지 메서드, `sort_entries`, `VisibilityFilter`, `WatchHandle` drop은 UI 스레드에서 불러도 된다.
- **늦게 온 결과:** 작업 스레드가 돌려주는 결과에는 요청 때 받은 세대 번호(`generation: u64`)가 붙는다. 탭의 현재 세대와 다르면 버린다. 빠르게 폴더를 옮겨 다녀도 앞 폴더의 목록이 뒤 폴더 화면을 덮지 않는다.
- **파일 변경은 작업 엔진으로:** 새 폴더, 이름 바꾸기, 삭제, 복사, 이동, 되돌리기는 모두 `JobEngine::submit`/`undo`/`redo`로 한다. UI 코드에서 `Provider::create_dir`·`rename`·`delete`나 `std::fs`의 쓰기 함수를 직접 부르지 않는다(설정 파일 저장과 인스턴스 잠금 파일은 예외).
- **사용자 문구:** 화면 문구는 한국어이고 `rf-ui/src/words.rs`(Task 2)와 `.slint` 파일에만 있다. 오류는 `Error`의 `Display`(영어 로그용)를 보여 주지 않고 `ErrorKind`·`FailureReason`·`IssueKind`를 `words.rs`가 한국어로 바꾼다(I18N-02). Project Fluent 도입은 정식 M1 UI 계획에서 한다.
- **Slint 참고 자료:** 요소·속성 정의는 `~/.cargo/registry/src/*/i-slint-compiler-1.18.1/builtin_elements.rs`, 표준 위젯은 같은 크레이트의 `widgets/fluent/*.slint`, 문법 예시는 `tests/syntax/`에 있다. 네트워크 문서(slint.dev)가 막혀 있으면 이것을 읽는다. 스타일은 `fluent`로 고정한다(`slint_build::CompilerConfiguration::new().with_style("fluent".into())`).
- **화면 확인:** Linux 컨테이너에서는 `scripts/xvfb-shot.sh <실행 파일> <출력.png> [인자...]`(Task 1)로 1440×900 화면을 찍는다. UI를 바꾼 태스크는 스크린샷 한 장 이상을 보고서에 경로로 남긴다. 컨테이너에는 `libxkbcommon-x11-0`, Xvfb, ImageMagick `import`, 한글 글꼴(WenQuanYi Zen Hei)이 있다.
- **헤드리스 테스트:** Slint 컴포넌트 테스트는 `i_slint_backend_testing::init_no_event_loop()`를 테스트 시작에서 부른다. 이벤트 루프가 필요한 흐름(작업 스레드 → UI)은 순수 상태 모듈 테스트로 대신 검증한다.
- **자원 제한:** 빌드·테스트는 `ulimit -v 16000000`과 `timeout`(빌드 1500초, 테스트 900초) 아래에서 돌린다. 첫 Slint 빌드는 의존성 400여 개를 컴파일하고 `target`이 2 GB 이상 늘어난다.

## Review Focus

- 10만 항목 폴더를 열거나 느린 네트워크 폴더를 여는 경우 → 목록은 작업 스레드에서 2000개 묶음으로 오고 정렬도 작업 스레드에서 하므로 창이 멈추지 않는다. 목록은 `ListView`(가상화)로 그린다. (Task 3 `listing_runs_off_the_ui_thread`, Task 4 스크린샷)
- A 폴더를 연 직후 B 폴더로 옮긴 경우 → 늦게 끝난 A의 목록·감시 이벤트·남은 공간이 B 화면을 덮지 않는다. (Task 3 `stale_listing_is_dropped`, `stale_changes_are_dropped`)
- 다른 앱이 지금 보는 폴더의 파일을 지우거나 이름을 바꾼 경우 → 목록이 증분 갱신되고 남은 선택·커서·스크롤 위치가 유지된다. 보던 폴더 자체가 사라지면 남아 있는 가장 가까운 상위 폴더로 옮긴다. (Task 3 `changes_keep_selection_by_name`, `vanished_folder_goes_to_nearest_parent`)
- 충돌 질문 창이 떠 있는 동안 창을 닫는 경우 → `JobEngine::shutdown`을 먼저 부르고 그 뒤에 남은 질문을 취소로 답한다. 그래야 그 작업이 사용자 취소가 아니라 끊긴 작업으로 저널에 남아 다음 실행 때 복구 창에 나온다(carry-m1-03 "quit order"). (Task 6 `close_shuts_engine_down_before_releasing_questions`)
- Linux처럼 휴지통이 없는 위치에서 Delete를 누른 경우 → 영구 삭제로 몰래 바꾸지 않는다. "휴지통을 쓸 수 없습니다. 영구 삭제할까요?" 확인을 거친다(OPS-03). (Task 6 `trash_refused_asks_before_permanent_delete`)
- 폴더를 자기 하위 폴더로 옮기려 하거나 이미 그 폴더에 있는 항목을 같은 곳으로 옮기려 하는 경우 → PICK이 그 대상을 고를 수 없게 표시하고 확인 버튼을 끈다(PICK-06, DND-09). (Task 7 `pick_rejects_source_and_descendants`)
- 앱을 두 번 실행한 경우 → 두 번째 실행은 같은 작업 저널을 열지 않고 "이미 실행 중입니다" 안내 후 끝난다(carry-m1-03 "single-instance journal guard"). (Task 6 `second_instance_is_refused`)
- 이름 바꾸기에서 이미 있는 이름을 입력한 경우 → 아무것도 덮어쓰지 않고 "같은 이름의 항목이 이미 있습니다"를 보여 준다. (Task 6 `rename_onto_existing_name_reports_and_keeps_both`)

---

## File Structure

```
Cargo.toml                              (수정) rust-version 1.92
README.md                               (수정) "데모 실행" 절(Task 10)
.github/workflows/ci.yml                (수정) Linux 패키지 설치, 시간 제한 45분(Task 1)
.github/workflows/demo.yml              (새로) OS별 데모 산출물(Task 10)
deny.toml                               (필요 시 수정) allow 항목 추가만
scripts/xvfb-shot.sh                    (새로) Linux 개발용 스크린샷 도구(Task 1)
installer/macos/Info.plist              (새로) 데모 .app 번들 정보(Task 10)
apps/racoon-finder/
  Cargo.toml
  src/main.rs                           인자 파싱, rf_ui::run 호출, Windows 릴리스는 콘솔 없음
crates/rf-ui/
  Cargo.toml
  build.rs                              slint_build::compile_with_config("ui/app.slint", fluent)
  ui/
    theme.slint                         Theme 전역(색, 간격, 글자 크기)
    icons.slint                         선 아이콘(Path) 모음
    widgets.slint                       ToolButton, TextButton, PrimaryButton, ProgressBar, Toast
    file-list.slint                     열 머리글 + 가상화 목록 행
    pane.slint                          탭 줄, 경로 줄(빵부스러기/편집), 목록, 상태 줄
    sidebar.slint                       즐겨찾기, 드라이브(용량 막대), 폴더 트리
    job-panel.slint                     작업 패널(진행, 일시정지·취소), 알림(toast)
    dialogs.slint                       이름 입력, 확인, 메시지(실패 목록·복구) 창
    conflict-dialog.slint               이름 충돌 창
    pick-dialog.slint                   이동/복사 위치 선택 모달
    app.slint                           MainWindow(위 요소 배치, 키 입력, 모달 겹침)
  src/
    lib.rs                              pub fn run, 모듈 선언
    format.rs                           크기·시간·종류 문구(순수)
    words.rs                            ErrorKind·FailureReason·IssueKind·Outcome 한국어 문구(순수)
    history.rs                          뒤로·앞으로 기록(순수)
    selection.rs                        선택·커서·앵커(순수)
    keys.rs                             단축키 → Command(순수)
    pane.rs                             탭·패널 상태, 목록 적용, 정렬, 숨김, 상태 줄 값(순수)
    loader.rs                           작업 스레드 목록·감시·남은 공간(세대 번호)
    places.rs                           즐겨찾기·드라이브 목록
    tree.rs                             지연 로딩 폴더 트리(순수 상태 + 로더 요청)
    pick.rs                             PICK 빠른 대상·유효성(순수)
    jobs.rs                             JobEngine 연결, 작업 패널 행, 알림
    resolver.rs                         UiResolver(질문을 UI로 보내고 답을 기다림)
    settings.rs                         settings.toml 읽기·쓰기(원자적 교체)
    instance.rs                         단일 실행 잠금
    open.rs                             OS 기본 앱으로 열기(opener)
    ui_bridge.rs                        Slint 구조체 변환, invoke_from_event_loop 도우미
    controller.rs                       콜백 연결과 명령 실행
  tests/
    ui_smoke.rs                         헤드리스 Slint 테스트
    loader_fs.rs                        임시 폴더로 로더·감시 통합 테스트
    jobs_flow.rs                        임시 폴더로 작업 흐름 통합 테스트
```

## 코어 API 요약 (이 계획이 부르는 것)

정확한 시그니처는 각 크레이트의 소스가 기준이다. 요약은 `feat/m1-core` 2ba9144 기준이다.

- `rf_core::VPath`: `local(path)`, `parse(&str)`, `to_uri()`(저장용), `as_local()`, `file_name()`, `parent()`, `join(name)`(이름 하나만), `starts_with(&base)`(구성요소 단위), `Display`(화면용, 손실 가능).
- `rf_core::Entry`: 공개 필드 `name: OsString`, `kind: EntryKind{File, Dir, Symlink, Other}`, `size`, `modified/created/accessed: Option<SystemTime>`, `attributes: Attributes`(HIDDEN, SYSTEM, READONLY …), `extension()`. 숨김 여부는 `attributes.contains(Attributes::HIDDEN)`(Unix의 점 파일 포함).
- `rf_core::{Error, ErrorKind}`: `error.kind()`로 문구를 고른다. `AppPaths::detect()`, `config_dir()`, `data_dir()`(작업 저널), `ensure_dirs()`, `mode()`(Installed/Portable).
- `rf_vfs::Vfs::with_local()`, `vfs.provider(&path)? -> Arc<dyn Provider>`. `Provider::list(dir, &CancelToken, &mut sink)`(2000개 이하 묶음, 순서 없음), `stat`, `stat_target`, `capabilities(path) -> Capabilities{WRITE, TRASH, WATCH}`, `space(path) -> Result<Option<DiskSpace>>`, `watch(dir, sink) -> WatchHandle`.
- `rf_vfs::{SortSpec, SortBy, SortKey{Name, Size, Modified, Kind}, SortOrder, sort_entries, compare_entries}`. 기본은 이름 오름차순, 폴더 먼저, 자연 정렬.
- `rf_vfs::VisibilityFilter{show_hidden, show_system}`, `is_visible`, `retain`.
- 감시: `WatchSink = Box<dyn FnMut(WatchEvent) + Send>`(백그라운드 스레드에서 불림), `WatchEvent{Changes(Vec<Change>), Rescan, Ended}`, `Change{Upsert(Entry), Removed(OsString), Renamed{from, to: Entry}}`. 감시를 먼저 시작하고 목록을 읽는다. 싱크 안에서 핸들을 drop하지 않는다.
- `rf_platform`: `KnownFolder::ALL`(Home, Desktop, Documents, Downloads, Pictures, Music, Videos)과 `known_folder(kf) -> Option<VPath>`, `disk_space(&VPath) -> Result<DiskSpace{total, free, available}>`(표시는 `available`), `trash_supported(&VPath)`(Linux는 항상 false). 드라이브·마운트 목록 함수는 없다(Task 3 `places.rs`가 만든다).
- `rf_jobs`: `EngineConfig::new(vfs, Arc<dyn Resolver>).with_journal(app_paths.data_dir())`, `JobEngine::start(config) -> (JobEngine, Receiver<JobEvent>)`. `submit(JobSpec) -> JobId`. `JobSpec{Transfer(TransferSpec), Delete(DeleteSpec), Rename{from, to}, Create{parent, name, kind}, Undo(..)}`. `TransferSpec::into_dir(TransferMode{Copy, Move}, &sources, &dest_dir, TransferOptions{conflict, errors, symlinks})`, `ConflictPolicy{Ask, Overwrite, Skip, OverwriteIfNewer, OverwriteIfSizeDiffers, AutoRename}`, `DeleteSpec{paths, method: DeleteMethod{Trash, Permanent}, errors}`. `subscribe_with_jobs() -> (Receiver<JobEvent>, Vec<JobSnapshot>)`, `JobEvent{State, Queue, Planned, Progress, Finished{id, report}, Recovered{jobs, temp_files_removed}, JournalError}`, `JobState{Queued, Paused, Planning, Running, Waiting, Finished(Outcome)}`, `Progress{done, total, current, bytes_per_sec, eta}`, `pause`/`resume`/`cancel`, `undo()`/`redo() -> Option<JobId>`(진행 중이면 None), `undo_history()`/`redo_history()`, `recovered()`, `resubmit_recovered(id)`, `dismiss_recovered(id)`, `shutdown(timeout) -> bool`(막힘). `Resolver` 트레잇의 `conflict`·`error`·`preflight`는 작업 스레드에서 불리고 답할 때까지 그 레인을 막는다. `JobReport{outcome, done, skipped, failures: Vec<ItemFailure{source, target, stage, kind, message, reason: Option<FailureReason>}>, outputs, undo, ..}`, `retry_spec()`.
- carry-m1-03의 UI 항목: (1) 종료 순서는 `shutdown` 먼저, 질문 해제는 그다음. (2) 복구된 작업을 다시 돌리면 이미 끝난 항목이 건너뛰기·번호 붙은 사본·NotFound로 나올 수 있으니 복구 창 문구가 그것을 미리 말한다. (3) "끝남"은 `JobEvent::Finished`의 `report.outcome`과 `skipped`로 판단하고 `done == total`로 판단하지 않는다. (4) 작업 목록은 `subscribe_with_jobs`와 `JobSnapshot.spec`으로 그린다. (5) 사용자 문구는 `FailureReason`에서 만든다. (6) 되돌리기·다시 실행은 한 번에 하나라서 진행 중이면 버튼을 끈다. (7) 저널 폴더 단일 실행 잠금은 UI가 한다.

## 화면 사양 (목업에서 옮김)

- 팔레트(`Theme` 전역, `dark`에 따라): 밝게 bg `#F6F7F9`, panel `#FFFFFF`, raised `#F0F2F5`, text `#1C2024`, muted `#5C6370`, faint `#8A919C`, border `#E1E4E8`, folder `#B7791F`, accent `#2F6FEB`, on-accent `#FFFFFF`, selection = accent 12%를 흰색에 섞은 `#E6EEFD`. 어둡게 bg `#15171A`, panel `#1D2024`, raised `#24282D`, text `#E7E9EC`, muted `#A0A7B1`, faint `#7D848E`, border `#2C3036`, folder `#E0A84A`, accent = 밝은 accent를 흰색과 35% 섞은 값(`#78A1F2`), on-accent `#0E1A33`, selection = panel과 accent 22% 혼합(`#313C51`). 알림 배경 밝게 `#1C2024`, 어둡게 `#2C3138`, 알림 글자 `#F2F4F7`, 알림 링크 `#9DBBFF`.
- 메인 창: 위 툴바(로고 아이콘, 뒤로·앞으로·위로 아이콘 버튼, 구분선, 새 폴더·복사…·이동…·삭제·이름 바꾸기·되돌리기 텍스트 버튼, 빈 공간, 보기 버튼 자리, 검색 입력 자리 "이 폴더에서 거르기"). 가운데는 왼쪽 사이드바(220~280px: "즐겨찾기", "이 PC" 드라이브와 용량 막대, 폴더 트리)와 두 패널(좌우, 같은 너비). 패널은 탭 줄(탭, + 새 탭), 경로 줄(조각 버튼과 `›`), 열 머리글(이름 ▲/▼, 크기, 수정한 날짜, 종류), 목록 행(높이 32px 데스크톱 밀도, 폴더 아이콘 folder 색, 파일 아이콘 muted 색, 선택 행 selection 배경), 상태 줄("항목 9개 · 선택 2개 (1.4 MB) · 남은 공간 212 GB"). 활성 패널은 accent 2px 테두리, 비활성은 border 1px. 아래는 작업 패널(작업이 있을 때만 높이를 가짐: 제목 "복사 중", 원본 → 대상, 진행 막대, "3 / 12 항목 · 48 MB/s · 남은 시간 약 20초 · 대기 중인 작업 1개", 일시정지/계속, 취소). 알림은 창 오른쪽 아래(작업 패널 위)에 뜬다: "항목 3개를 이동했습니다 [되돌리기] [×]", 5초 뒤 사라짐.
- PICK 모달: 어두운 막(scrim) 위 가운데 최대 1040px 창. 머리: 제목 "어디로 이동할까요?"/"어디로 복사할까요?", 요약 "항목 3개 · 1.2 GB"(폴더가 있으면 "파일 1.2 MB와 폴더 1개"), 원본 이름 칩(최대 3개, 나머지는 "외 N개"), 이동/복사 토글, 닫기. 경로 입력("대상 경로")과 새 폴더 버튼. 왼쪽 폴더 트리와 "폴더 이름으로 거르기" 입력, 오른쪽 빠른 대상 묶음("반대편 패널", "열린 탭", "최근 위치", "즐겨찾기"), 고를 수 없는 대상은 faint 색과 이유("옮기는 항목 안이라 고를 수 없음"). 아래: "이름이 겹치면" 물어보기/덮어쓰기/건너뛰기/이름 바꾸기, 남은 공간 문구, 취소, "여기로 이동"/"여기로 복사".
- 충돌 창(760×560): "같은 이름의 파일이 이미 있습니다", 설명 "‘설계안.pdf’가 C:\…\백업 에 이미 있습니다.", 두 카드(옮겨 올 파일/이미 있는 파일: 이름, 위치, 크기, 수정한 날짜, 더 최근 쪽에 "더 최근" 표시), 선택지 "덮어쓰기"(있던 파일을 옮겨 올 파일로 바꿉니다), "건너뛰기"(이 파일은 옮기지 않습니다), "이름 바꿔 둘 다 두기", 체크박스 "남은 충돌에도 같은 선택 적용", "작업 취소", "계속". 폴더끼리 충돌이면 덮어쓰기 대신 "합치기"로 쓴다(rf-jobs는 두 폴더를 합친다).
- 키보드(Windows·Linux는 Ctrl, macOS는 Slint가 Cmd를 `control`로 준다): Tab 패널 전환, Enter 열기, Backspace·Alt+↑ 상위 폴더, Alt+←/→ 뒤로/앞으로, ↑↓ Home End PageUp PageDown 커서(Shift면 범위 선택), Ctrl+Space 커서 항목 선택 토글, Ctrl+A 전체 선택, Esc 선택 해제(모달에서는 닫기), F2 이름 바꾸기, Delete 휴지통, Shift+Delete 영구 삭제(확인), F5 복사…, F6 이동…, Ctrl+Shift+N·F7 새 폴더, Ctrl+Z 되돌리기, Ctrl+Y·Ctrl+Shift+Z 다시 실행, Ctrl+L 경로 편집, Ctrl+T 새 탭, Ctrl+W 탭 닫기, Ctrl+H 숨김 파일 토글, 그 밖의 글자 입력은 이름 앞부분으로 이동(NAV-14, 1초 안의 입력은 이어 붙임).

## 이 계획이 일부러 다루지 않는 PRD 항목

정식 M1 UI 계획과 M1 Windows 플랫폼 통합 계획으로 넘긴다: 상하 분할과 분할선 끌기(LAY-02), 패널별 트리(LAY-03 b), 여러 창(LAY-07 일부), 레이아웃 저장(LAY-08), 작업 패널 분리 창(LAY-09 일부), 주소창 자동 완성·▸ 하위 목록(NAV-03 일부), 마우스 4/5번(NAV-04 일부), 목록 외 보기 모드·썸네일(NAV-06), 추가 열과 열 너비 조절(NAV-07 일부), 그룹 헤더, 폴더별 보기 설정(NAV-10), 가상 위치(휴지통 보기, 네트워크), 클라우드 상태(NAV-17), 네이티브 셸 메뉴(OPS-05), 속성 창, 경로 복사(클립보드), 링크 만들기, 체크섬, 터미널 열기, OS와 주고받는 드래그 앤 드롭과 클립보드(DND-05, DND-10), 길게 누르기(PICK-02), Project Fluent, 설치 프로그램, 서명·공증.

---

### Task 1: 크레이트 뼈대, Slint 빌드, 테마, CI

**Files:** `Cargo.toml`, `crates/rf-ui/{Cargo.toml,build.rs,src/lib.rs,ui/theme.slint,ui/icons.slint,ui/widgets.slint,ui/app.slint}`, `crates/rf-ui/tests/ui_smoke.rs`, `apps/racoon-finder/{Cargo.toml,src/main.rs}`, `.github/workflows/ci.yml`, `scripts/xvfb-shot.sh`

- [ ] **Step 1:** 루트 `Cargo.toml`의 `rust-version`을 `"1.92"`로 바꾼다. `docs/superpowers/plans/README.md` 표에 이 계획을 6번째 줄로 더하고 "아직 없는 계획"의 M1 UI 셸 항목에 "데모 범위는 M1-05가 먼저 구현했다"를 덧붙인다.
- [ ] **Step 2:** `crates/rf-ui`를 만든다(M1-01의 상속 블록, `[lints] workspace = true`). `cargo add -p rf-ui slint@1.18.1`, `cargo add -p rf-ui --build slint-build@1.18.1`, `cargo add -p rf-ui --dev i-slint-backend-testing@1.18.1`, 경로 의존 `rf-core`, `rf-vfs`, `rf-platform`, `rf-jobs`. `build.rs`는 `slint_build::compile_with_config("ui/app.slint", CompilerConfiguration::new().with_style("fluent".into()))`. `lib.rs`는 `slint::include_modules!()`를 `mod generated`(공개 재수출: `pub use generated::*`)로 감싸고, 생성된 코드가 워크스페이스 린트(`unsafe_code` deny, clippy pedantic, `missing_debug_implementations`, `unused_qualifications`, `rust_2018_idioms`)에 걸리면 그 모듈에만 `#[allow(…, reason = "generated by slint-build")]`를 단다(필요한 것만, 손으로 쓴 코드에는 쓰지 않음). `pub fn run(start: Option<std::path::PathBuf>) -> Result<(), RunError>`를 둔다. `RunError`는 `thiserror`로 `Platform(slint::PlatformError)`, `Paths(rf_core::Error)` 두 갈래.
- [ ] **Step 3:** `ui/theme.slint`에 화면 사양의 팔레트를 `export global Theme`로 쓴다: `in-out property <bool> dark`, `in-out property <color> accent-base: #2F6FEB`, 나머지는 `out property`로 계산(`accent`는 `dark`일 때 `accent-base.mix(#FFFFFF, 0.65)`처럼 Slint의 `mix`로). 간격 상수(`gap-xs: 4px` … `gap-l: 16px`), 글자 크기(`font-small: 12px`, `font-body: 14px`, `font-title: 20px`), 행 높이(`row-height: 32px`), 모서리(`radius: 8px`).
- [ ] **Step 4:** `ui/icons.slint`에 `export component Icon inherits Path`를 두고 `in property <string> name`으로 목업의 SVG 경로(뒤로, 앞으로, 위로, 새 폴더, 복사, 이동, 삭제, 이름 바꾸기, 되돌리기, 목록 보기, 검색, 폴더, 파일, 드라이브, 별, 더하기, 닫기, 일시정지, 재생, 체크, 정렬 위·아래, 펼침 ›·⌄)를 `commands` 문자열로 고른다. 24×24 viewbox, `stroke-width: 1.8px`, 채움 없음, 색은 `in property <brush> color`.
- [ ] **Step 5:** `ui/widgets.slint`에 `ToolButton`(아이콘만, 32×32, 호버 배경 raised, `accessible-role: button`, `accessible-label` 필수), `TextButton`(아이콘+글자), `PrimaryButton`(accent 배경, on-accent 글자, `enabled: false`면 border 배경과 faint 글자), `ProgressBar`(0..1), `Toast`(메시지, 선택 동작 버튼, 닫기)를 만든다. 각 버튼은 `FocusScope`가 아니라 `TouchArea` + `accessible-*` 속성으로 만들고 키보드 포커스는 창 단위 키 처리(Task 4)가 맡는다.
- [ ] **Step 6:** `ui/app.slint`에 `export component MainWindow inherits Window`를 두고 일단 툴바와 빈 본문(“여기에 패널이 들어갑니다” 대신 빈 panel 두 개)만 그린다. `in property <string> app-title`을 `title`에 묶는다. 기본 크기 1280×800, 최소 900×560. 배경 `Theme.bg`, 기본 글꼴 크기 `Theme.font-body`.
- [ ] **Step 7:** `apps/racoon-finder` 실행 파일 크레이트: `cargo add -p racoon-finder --path crates/rf-ui`. `main.rs`는 첫 줄 `#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]`, 첫 인자를 시작 폴더로 넘겨 `rf_ui::run`을 부르고 실패하면 `eprintln!`(이 파일만 `print_stderr` 허용 `#[expect]`) 후 종료 코드 1.
- [ ] **Step 8:** 헤드리스 테스트 `tests/ui_smoke.rs`: `main_window_builds_with_theme` — `init_no_event_loop()` 뒤 `MainWindow::new()`가 성공하고 `Theme`의 `dark`를 바꾸면 `bg`가 `#15171A`가 된다(`window.global::<Theme>().get_bg()`).
- [ ] **Step 9:** `.github/workflows/ci.yml`의 `test` 잡에 `if: runner.os == 'Linux'`인 단계 `sudo apt-get update && sudo apt-get install -y libfontconfig1-dev libxkbcommon-dev libxkbcommon-x11-0`를 checkout 다음에 넣고 `timeout-minutes`를 45로 올린다.
- [ ] **Step 10:** `scripts/xvfb-shot.sh`(실행 권한): Xvfb `:99`를 1440×900×24로 띄우고 `SLINT_BACKEND=winit-software`로 앱을 실행해 4초 뒤 `import -window root`로 찍은 다음 둘 다 끝낸다. 인자: `<실행 파일> <출력.png> [앱 인자...]`. 앱 로그는 `<출력.png>.log`.
- [ ] **Step 11:** 검사: 표준 검사, `cargo test -p rf-ui`, 교차 clippy(Windows·macOS 타깃), `cargo deny check`(실패하면 원인 크레이트의 라이선스가 GPL-3.0 호환인지 확인해 `deny.toml` `allow`에 한 줄 추가), 스크린샷 1장. 커밋: `feat(rf-ui): scaffold the Slint UI crate and the app binary`.

### Task 2: 순수 모듈 — 문구, 기록, 선택, 단축키

**Files:** `crates/rf-ui/src/{format.rs,words.rs,history.rs,selection.rs,keys.rs}`

모두 Slint를 쓰지 않는다. TDD: 테스트를 먼저 쓰고 실패를 확인한 뒤 구현한다.

- [ ] **format.rs:** `format_size(bytes: u64) -> String`: 1024 미만은 `"{n} 바이트"`, 그 위는 KB·MB·GB·TB(1024 단위), 10 미만은 소수 한 자리(`1.4 MB`, 끝의 `.0`은 지움), 10 이상은 반올림 정수(`214 KB`). `format_time(t: SystemTime) -> String`: 로컬 시간대 `YYYY-MM-DD HH:MM`(`cargo add -p rf-ui jiff`, `jiff::Zoned::try_from(t)`; 실패하면 UTC). 테스트는 시간대에 기대지 않게 `format_time_in(t, &jiff::tz::TimeZone)`을 따로 두고 UTC·Asia/Seoul로 검사. `kind_label(entry: &Entry) -> String`: 폴더 "파일 폴더", 링크 "바로 가기", 기타 "특수 파일", 확장자 표(대소문자 무시): pdf "PDF 문서", txt "텍스트 문서", md "Markdown 문서", png/jpg/jpeg/gif/webp/heic/bmp/tiff → "PNG 이미지"처럼 대문자 확장자 + " 이미지", mp3/wav/flac/m4a → " 오디오", mp4/mov/mkv/avi → " 동영상", zip/7z/rar/tar/gz → " 압축 파일", doc/docx "Word 문서", xls/xlsx "Excel 통합 문서", ppt/pptx "PowerPoint 프레젠테이션", hwp/hwpx "한글 문서", exe "응용 프로그램", app "응용 프로그램", 그 밖은 `"{EXT} 파일"`, 확장자가 없으면 "파일". 테스트: `sizes_round_like_the_mockup`(0, 1023, 1024, 1_477_427 → `1.4 MB`, 219_136 → `214 KB`, 13_002_342 → `12 MB`), `times_use_the_given_zone`, `kinds_cover_common_extensions`.
- [ ] **words.rs:** `error_kind(ErrorKind) -> &'static str`(NotFound "찾을 수 없습니다", PermissionDenied "권한이 없습니다", AlreadyExists "같은 이름의 항목이 이미 있습니다", StorageFull "디스크 공간이 부족합니다", ReadOnly "읽기 전용 위치입니다", Busy "다른 프로그램이 사용 중입니다", … 모든 변형, `_`는 "알 수 없는 오류가 났습니다"), `failure_reason(&FailureReason) -> String`(모든 변형을 사용자 말로: IntoItself "폴더를 자기 안으로 옮길 수 없습니다", TrashRefused "휴지통으로 보낼 수 없습니다", OtherVolume "다른 드라이브의 항목이라 건너뛰었습니다", ChangedSinceCopy "복사한 뒤 바뀌어 되돌리지 않았습니다" …), `issue_kind(&IssueKind) -> String`, `outcome(Outcome, kind: JobKind) -> String`, `job_title(kind, state)`("복사 중", "이동 중", "휴지통으로 보내는 중", "삭제 중", "이름 바꾸는 중", "만드는 중", "되돌리는 중", 일시정지면 "일시정지됨", 대기면 "대기 중"). 테스트: `every_error_kind_has_words`(알려진 변형 전부 빈 문자열이 아니고 영어 알파벳만으로 된 문구가 없음), `every_failure_reason_has_words`, `job_titles_follow_state`.
- [ ] **history.rs:** `History { back: Vec<VPath>, forward: Vec<VPath> }`, `visit(&mut self, from: VPath)`(forward 비움, 최대 50), `go_back(&mut self, current: VPath) -> Option<VPath>`, `go_forward(..)`, `can_back()`, `can_forward()`. 테스트: `back_and_forward_round_trip`, `visiting_clears_forward`, `history_is_capped`.
- [ ] **selection.rs:** 이름(`OsString`) 기준 선택. `Selection { selected: HashSet<OsString>, anchor: Option<usize>, cursor: Option<usize> }`와 메서드 `click(i, names)`(그 하나만), `toggle(i, names)`(Ctrl), `extend_to(i, names)`(Shift: 앵커~i), `move_cursor(delta, extend, len, names)`, `select_all(names)`, `clear()`, `invert(names)`, `reconcile(old_names, new_names)`(목록이 바뀐 뒤 남은 이름만 유지하고 커서는 같은 이름, 그 이름이 사라졌으면 같은 위치에 가장 가까운 항목), `indices(names) -> Vec<usize>`. 테스트: `shift_click_selects_range_from_anchor`, `ctrl_click_toggles`, `reconcile_keeps_names_and_moves_cursor`, `invert_and_all`.
- [ ] **keys.rs:** `Mods { ctrl, shift, alt }`, `enum Command { SwitchPane, Open, Up, Back, Forward, CursorUp, CursorDown, CursorHome, CursorEnd, PageUp, PageDown, ToggleSelect, SelectAll, ClearSelection, Rename, Trash, DeletePermanently, CopyTo, MoveTo, NewFolder, Undo, Redo, EditPath, NewTab, CloseTab, ToggleHidden, TypeToSelect(String) }`, `fn command_for(key: &str, mods: Mods) -> Option<Command>`. `key`는 Slint가 주는 문자열이며 특수 키는 `slint::platform::Key`를 `char`로 바꾼 값과 비교한다(`String::from(Key::F2)` 등). Shift 커서 이동은 `CursorDown`과 `mods.shift`를 함께 돌려주도록 `Command` 대신 `(Command, Mods)`를 돌려줘도 된다(구현자가 고르고 테스트로 고정). 테스트: 화면 사양의 단축키 표 전부.
- [ ] 검사·커밋: `feat(rf-ui): add the formatting, wording, history, selection and key modules`.

### Task 3: 패널 상태, 목록 로더, 장소

**Files:** `crates/rf-ui/src/{pane.rs,loader.rs,places.rs}`, `crates/rf-ui/tests/loader_fs.rs`

- [ ] **pane.rs (순수):** `Tab { location: VPath, history: History, all: Vec<Entry>, visible: Vec<Entry>, sort: SortSpec, filter: VisibilityFilter, selection: Selection, generation: u64, status: LoadStatus{Loading, Ready, Failed(ErrorKind)}, free: Option<u64>, type_buffer: (String, Instant) }`, `Pane { tabs: Vec<Tab>, active: usize }`. 메서드: `navigate(&mut self, to: VPath) -> LoadRequest{generation, path}`(기록 남김, 세대 +1, 목록 비우고 Loading), `reload() -> LoadRequest`, `apply_batch(gen, Vec<Entry>) -> bool`(세대가 다르면 false; 묶음을 더하되 정렬·필터는 `finish`에서 한 번), `finish(gen, Result<(), ErrorKind>)`, `apply_changes(gen, Vec<Change>)`(Upsert는 같은 이름 교체·없으면 `compare_entries` 이진 탐색 삽입, Removed·Renamed 처리 후 `selection.reconcile`), `set_sort(key)`(같은 키면 순서 반전), `set_show_hidden(bool)`, `open_target(i) -> Option<OpenAction{Navigate(VPath), OpenFile(VPath)}>`(폴더·폴더 링크는 이동, 나머지는 파일 열기), `status_line() -> String`(화면 사양 형식, 선택 크기는 파일 크기 합만, 폴더만 선택하면 크기 생략), `type_to_select(text, now)`. 정렬이 무거우므로 `finish`는 목록 전체를 작업 스레드에서 정렬해 넘기는 길(`apply_sorted(gen, Vec<Entry>)`)도 둔다. 테스트: `navigate_bumps_generation`, `stale_listing_is_dropped`, `stale_changes_are_dropped`, `changes_keep_selection_by_name`, `sort_toggles_on_same_key`, `hidden_toggle_refilters`, `status_line_matches_mockup`, `type_to_select_jumps_by_prefix`.
- [ ] **loader.rs:** `Loader`는 `Vfs`를 들고 `request(req: LoadRequest, sink: impl Fn(LoadEvent) + Send + 'static) -> LoadTicket`을 제공한다. 작업 스레드에서: (1) `capabilities`에 WATCH가 있으면 감시 시작(싱크는 받은 `Change` 묶음을 `LoadEvent::Changes{generation, changes}`로, `Rescan`은 `LoadEvent::Rescan{generation}`, `Ended`는 `LoadEvent::Vanished{generation}`), (2) `list`로 묶음을 모아 `sort_entries` 후 `LoadEvent::Listed{generation, entries}` 한 번, 실패는 `LoadEvent::Failed{generation, kind}`, (3) `space`로 `LoadEvent::Space{generation, available}`. `LoadTicket`이 drop되거나 `cancel()`되면 `CancelToken`을 취소하고 감시 핸들을 놓는다(UI 스레드에서 drop해도 됨: 핸들 drop은 빠르다). 감시 핸들은 티켓이 갖고, 싱크 안에서 drop하지 않는다. `Vanished`를 받은 UI는 `nearest_existing_parent(path)`(작업 스레드에서 `stat`)로 옮긴다. 이벤트 전달은 테스트에서 `mpsc` 채널, 앱에서 `invoke_from_event_loop`를 쓰도록 싱크를 클로저로 받는다. 통합 테스트(`tests/loader_fs.rs`, tempfile): `listing_runs_off_the_ui_thread`(싱크가 호출된 스레드 ID가 테스트 스레드와 다름), `lists_and_sorts_a_folder`, `watch_reports_new_and_removed_files`(최대 5초 대기 폴링), `cancel_stops_events`, `vanished_folder_goes_to_nearest_parent`.
- [ ] **places.rs:** `Place { label: String, path: VPath, kind: PlaceKind{Favorite(KnownFolder), Drive}, space: Option<DiskSpace> }`. `favorites()`: `KnownFolder::ALL` 순서로 `known_folder`가 준 것만, 이름 홈·바탕 화면·문서·다운로드·사진·음악·동영상. `drives()`: Windows는 `A:`~`Z:` 중 `std::fs::metadata("X:\\")`가 되는 것(라벨 `C:`는 "로컬 디스크 (C:)", 나머지 "드라이브 (D:)"), macOS는 `/`("Macintosh HD" 대신 "시스템 디스크")와 `/Volumes/*` 중 `/`와 같은 장치가 아닌 폴더, Linux는 `/`, `/media/*/*`, `/run/media/*/*`, `/mnt/*`. 모두 작업 스레드에서 불리는 함수로 문서화하고, 남은 공간은 `rf_platform::disk_space`로 채운다. 테스트: `favorites_are_in_sidebar_order`, `linux_drives_include_root`(cfg linux).
- [ ] 검사·커밋: `feat(rf-ui): add pane state, the folder loader and places`.

### Task 4: 메인 창 — 패널, 목록, 사이드바 장소, 키보드

**Files:** `crates/rf-ui/ui/{file-list.slint,pane.slint,sidebar.slint,app.slint}`, `crates/rf-ui/src/{ui_bridge.rs,controller.rs,open.rs,lib.rs}`, `crates/rf-ui/tests/ui_smoke.rs`

- [ ] **Slint 구조체:** `FileRow { name: string, size: string, modified: string, kind: string, is-dir: bool, is-link: bool, selected: bool, hidden: bool }`, `TabItem { title: string, active: bool }`, `Crumb { label: string, index: int }`, `PaneView { tabs: [TabItem], crumbs: [Crumb], path-text: string, rows: [FileRow], cursor: int, sort-key: int, sort-ascending: bool, status: string, loading: bool, error: string, active: bool }`, `PlaceRow { label: string, info: string, fill: float, is-drive: bool }`. 목록은 행이 많으므로 `PaneView.rows`는 Rust의 `Rc<VecModel<FileRow>>`를 그대로 넘기고(전체 교체 대신 바뀐 행만 `set_row_data`), 나머지는 값으로 넘긴다.
- [ ] **file-list.slint:** 머리글(이름·크기·수정한 날짜·종류, 현재 정렬 열에 위/아래 아이콘, 누르면 `sort(col)`), `ListView`로 행을 그린다. 행 `TouchArea`: `pointer-event`에서 `event.modifiers`를 읽어 `row-clicked(index, ctrl, shift)`, `double-clicked`에서 `row-activated(index)`. 오른쪽 클릭은 Task 6에서 메뉴로 바뀐다. 커서 행은 1px accent 테두리, 선택 행은 `Theme.selection` 배경, 숨김 항목은 글자 50% 투명. 커서가 보이도록 `ListView.viewport-y`를 Rust가 요청할 때 맞추는 `scroll-to(index)` 함수.
- [ ] **pane.slint:** 탭 줄(탭 클릭 `tab-selected(i)`, 가운데 클릭이나 × `tab-closed(i)`, + `tab-added()`), 경로 줄(조각 버튼 `crumb-clicked(index)`, 빈 곳 클릭이나 Ctrl+L이면 `LineEdit`로 바뀜, Enter `path-entered(text)`, Esc 취소), 오류 상태면 목록 대신 가운데에 `error` 문구와 "다시 시도" 버튼, 로딩 중이면 빈 목록 위에 "불러오는 중…"(0.3초 지연 후에만 보이게), 상태 줄. 패널 아무 곳이나 누르면 `activated()`.
- [ ] **sidebar.slint:** "즐겨찾기"와 "이 PC" 머리, 장소 행(폴더 아이콘/드라이브 아이콘, 이름, 드라이브는 용량 막대와 "128 GB 남음 / 512 GB"), 누르면 `place-clicked(i)`(활성 패널을 그 위치로). 트리 자리는 Task 5가 채운다.
- [ ] **app.slint:** 툴바·사이드바·두 패널·(빈) 작업 패널을 화면 사양대로 배치. 창 전체를 덮는 `FocusScope`가 `key-pressed(event)`에서 `key(event.text, ctrl, shift, alt) -> bool` 콜백을 부르고 Rust가 처리했으면 `accept`. 텍스트 입력(LineEdit)에 포커스가 있을 때는 그쪽이 먼저 받는다. 툴바 버튼은 `command(name)` 콜백 하나로 모은다.
- [ ] **controller.rs:** `AppState { panes: [Pane; 2], active: usize, loader, places, show_hidden }`를 `Rc<RefCell<_>>`로 UI 스레드에 둔다. `ui_bridge::post(f)`는 `slint::invoke_from_event_loop`로 UI 스레드에서 `f(&Controller)`를 부른다(컨트롤러는 `thread_local!`에 둔다). 시작: 왼쪽 = 시작 인자 폴더 또는 홈, 오른쪽 = 문서(없으면 홈). 키·툴바·행 콜백을 `keys::command_for`와 `Command` 실행으로 연결한다(이 태스크는 탐색·선택·정렬·탭·숨김 명령까지, 파일을 바꾸는 명령은 Task 6). `Open`: 폴더면 이동, 파일이면 `open::open_file(path)`(작업 스레드에서 `opener::open`, `cargo add -p rf-ui opener`; 실패는 상태 줄 대신 알림으로 "열 수 없습니다: {words}").
- [ ] 테스트(`ui_smoke.rs`): `pane_view_shows_rows_from_the_model`(가짜 `PaneView`를 넣고 `get_` 값 확인), `key_callback_reaches_rust`(`invoke_key`가 등록한 Rust 클로저를 부름), `sort_header_invokes_callback`. 스크린샷: 홈 폴더와 `/usr/share`를 연 화면(밝게·어둡게 각 1장).
- [ ] 검사·커밋: `feat(rf-ui): show two live folder panes with tabs, places and keyboard navigation`.

### Task 5: 공유 폴더 트리

**Files:** `crates/rf-ui/src/tree.rs`, `crates/rf-ui/ui/sidebar.slint`, `controller.rs`

- [ ] **tree.rs (순수 상태 + 요청):** 루트는 장소(즐겨찾기, 드라이브). `TreeNode { path, label, depth, expanded, children: Option<Vec<NodeId>>, loading }`, 평탄화한 보이는 행 `rows() -> Vec<TreeRowData { label, depth, expanded, has_children_unknown, selected }>`, `toggle(id) -> Option<LoadRequest>`(처음 펼칠 때만 로더 요청: 폴더만, 숨김은 설정 따름, 자연 정렬), `apply_children(gen, Vec<Entry>)`, `reveal(path) -> Vec<LoadRequest>`(활성 패널 위치의 조상 노드를 펼치고 그 노드를 선택: LAY-03 a 공유 트리). 테스트: `expanding_requests_children_once`, `reveal_expands_ancestors`, `stale_children_are_dropped`.
- [ ] **UI:** 사이드바 아래쪽에 트리 행(들여쓰기 `depth × 16px`, ›/⌄ 펼침 아이콘, 폴더 아이콘, 선택 행 강조). 행 클릭은 활성 패널 이동, 펼침 아이콘 클릭은 펼치기만. 활성 패널이 바뀌거나 위치가 바뀌면 `reveal`.
- [ ] 스크린샷 1장, 검사·커밋: `feat(rf-ui): add the shared folder tree that follows the active pane`.

### Task 6: 작업 엔진 연결 — 파일 작업, 작업 패널, 충돌, 되돌리기

**Files:** `crates/rf-ui/src/{jobs.rs,resolver.rs,instance.rs,controller.rs}`, `crates/rf-ui/ui/{job-panel.slint,dialogs.slint,conflict-dialog.slint,file-list.slint,app.slint}`, `crates/rf-ui/tests/jobs_flow.rs`

- [ ] **instance.rs:** `InstanceLock::acquire(dir: &Path) -> Result<InstanceLock, InstanceError{AlreadyRunning, Io}>`: `dir/instance.lock`을 열고 `std::fs::File::try_lock()`(Rust 1.89 안정). 잠금은 값이 살아 있는 동안 유지. 두 번째 실행은 작은 Slint 안내 창("이미 실행 중입니다. 먼저 연 창을 쓰세요.") 후 종료. 테스트: `second_instance_is_refused`.
- [ ] **resolver.rs:** `UiResolver`는 `Resolver`를 구현한다. `conflict`는 질문 `Question::Conflict{job, conflict}`를 `post`로 UI에 보내고 `mpsc::Receiver<ConflictAnswer>`에서 기다린다. `closing: AtomicBool`이 켜져 있거나 채널이 끊기면 `ConflictAnswer::once(ConflictAction::Cancel)`. `error`는 이 데모에서 쓰지 않는 `ErrorPolicy::Ask`일 때만 오므로 `Skip`. `preflight`는 이슈 목록을 UI에 보여 주고 "계속"(Proceed)/"막힌 항목 건너뛰기"(SkipBlocked)/"취소"를 받는다. `release_all()`은 기다리는 질문을 모두 취소로 답한다. 테스트(`jobs_flow.rs`): `conflict_question_round_trips`(가짜 post가 즉시 답함), `release_answers_cancel`.
- [ ] **jobs.rs:** `Jobs::start(app_paths, post) -> (Jobs, recovered_later)`: `EngineConfig::new(Vfs::with_local(), resolver).with_journal(app_paths.data_dir())`, `JobEngine::start`, 다리 스레드가 첫 수신기를 비우며 `post`로 UI에 넘긴다(이후 패널은 `subscribe_with_jobs`의 스냅샷으로 초기화). UI 쪽 상태 `JobRows`: 진행 중인 작업마다 제목(`words::job_title`), 설명(원본 이름 또는 "항목 N개" → 대상 폴더 이름), 진행률(`done.bytes/total.bytes`, 바이트가 0이면 항목 수), 진행 문구("3 / 12 항목 · 48 MB/s · 남은 시간 약 20초"), 상태(대기·일시정지·실행·질문 대기). `Finished`에서 행을 지우고: 결과가 Completed이고 종류가 이동·복사·휴지통이면 알림 "항목 N개를 이동했습니다 [되돌리기]"(복사 "복사했습니다", 휴지통 "휴지통으로 보냈습니다", 5초), CompletedWithErrors나 Aborted면 알림 "항목 N개를 처리하지 못했습니다 [자세히]"(자세히 = 실패 목록 창: 항목 경로와 `words::failure_reason` 또는 `words::error_kind`), Cancelled는 알림 없음. 되돌리기 버튼은 `engine.undo()`, 그 작업의 `Finished`는 알림 "되돌렸습니다". 툴바 되돌리기·다시 실행 버튼은 `undo_history()`가 비었거나 되돌리기 작업이 진행 중이면 끈다.
- [ ] **명령:** NewFolder → 이름 입력 창(기본 "새 폴더", 확인 시 `JobSpec::Create{parent, name, kind: Dir}`, 끝나면 `report.outputs[0]`을 선택). Rename → 이름 입력 창(현재 이름, 확장자 앞까지 선택; 빈 이름·같은 이름은 확인 끔, 쓸 수 없는 문자(`/`, Windows는 `\ : * ? " < > |`)는 입력 중 경고), `JobSpec::Rename{from, to}`, 실패(AlreadyExists 등)는 알림. Trash → 선택 항목의 위치가 `trash_supported`(작업 스레드에서 확인)면 `DeleteSpec{method: Trash}` 바로 제출, 아니면 확인 창 "휴지통을 쓸 수 없는 위치입니다. 항목 N개를 영구 삭제할까요? 되돌릴 수 없습니다." 후 Permanent. 휴지통 작업이 `TrashRefused`로 실패한 항목이 있으면 같은 확인 뒤 `retry_spec()`의 방법을 Permanent로 바꿔 제출. DeletePermanently → 같은 확인 창 후 Permanent. CopyTo/MoveTo는 Task 7.
- [ ] **오른쪽 클릭 메뉴:** `file-list.slint`의 행에 `ContextMenuArea`(Slint 1.18)로 "열기", "이동…", "복사…", "이름 바꾸기", "휴지통으로 보내기", "영구 삭제", "새 폴더"(빈 곳). 메뉴 항목은 `command(name)`으로 같은 명령을 쓴다(OPS-05의 앱 메뉴 부분, 맨 위는 이동…·복사…).
- [ ] **충돌 창:** `conflict-dialog.slint`를 화면 사양대로. Rust가 `ConflictView { file-name, target-dir, incoming-size, incoming-time, existing-size, existing-time, incoming-newer: bool, folders: bool, remaining: int }`를 채우고 사용자의 선택(`overwrite|skip|rename|cancel`, 모두 적용)을 `ConflictAnswer`로 돌려보낸다. 창이 떠 있는 동안 다른 키는 막는다(모달 겹침 + 막 `TouchArea`).
- [ ] **복구:** 시작 후 `Recovered`가 오면 작업이 있을 때만 메시지 창 "지난번에 끝나지 않은 작업이 N개 있습니다" + 작업별 설명(`JobSnapshot.spec`처럼 `IncompleteJob.spec`에서) + "다시 실행"(resubmit) / "버리기"(dismiss). 문구에 "이미 끝난 항목은 건너뛰거나 번호를 붙인 사본이 생길 수 있습니다"를 넣는다(carry 2).
- [ ] **종료:** 창 닫기(`window().on_close_requested`)에서 작업이 진행 중이면 확인 창 "진행 중인 작업 N개가 있습니다. 끝내면 다음 실행 때 이어서 할 수 있습니다." → 끝내기면 `closing` 켜기 → 작업 스레드에서 `engine.shutdown(Duration::from_secs(5))` → 그 뒤 `resolver.release_all()` → UI 스레드에서 `slint::quit_event_loop()`. 테스트: `close_shuts_engine_down_before_releasing_questions`(가짜 resolver·엔진 래퍼로 호출 순서 기록).
- [ ] 통합 테스트(`jobs_flow.rs`, tempfile, `NonInteractive` 또는 자동 응답 resolver): `create_folder_selects_output`, `rename_onto_existing_name_reports_and_keeps_both`, `trash_refused_asks_before_permanent_delete`(Linux: 휴지통 불가 → 확인 요청 이벤트가 나오고 확인 전에는 파일이 그대로), `move_then_undo_restores`, `undo_disabled_while_running`.
- [ ] 스크린샷: 복사 진행 중 작업 패널, 충돌 창, 알림(각 1장, 큰 파일 복사로 재현). 검사·커밋: `feat(rf-ui): run file operations through the job engine with conflicts, undo and recovery`.

### Task 7: PICK 모달

**Files:** `crates/rf-ui/src/pick.rs`, `crates/rf-ui/ui/pick-dialog.slint`, `controller.rs`, `settings.rs`(최근 대상만, Task 8이 나머지)

- [ ] **pick.rs (순수):** `PickRequest { mode: TransferMode, sources: Vec<VPath>, file_bytes: u64, folder_count: usize, from_dir: VPath }`. `summary(&req) -> String`("항목 3개 · 1.2 GB", 폴더가 있으면 "항목 3개 · 파일 1.2 MB와 폴더 1개"). `quick_targets(req, opposite: &VPath, open_tabs: &[VPath], recent: &[VPath], favorites: &[Place]) -> Vec<TargetGroup{title, items: Vec<Target{path, note, validity}>}>`(반대편 패널 → 열린 탭 → 최근 10개 → 즐겨찾기, 중복 제거, 반대편 패널이 기본 선택: PICK-05 기본값). `validate(req, target) -> Validity{Ok, SameFolder, IntoSource, NotAFolder, NotWritable, NotEnoughSpace{need, available}}`: 대상이 원본 자신이거나 원본 아래면 IntoSource(고를 수 없음), 이동인데 대상이 `from_dir`이면 SameFolder(고를 수 없음, DND-09), 쓰기·공간은 작업 스레드에서 얻은 `Capabilities`·`available`로 경고만(확인은 가능, PICK-06 "확인 전에 경고"). 테스트: `pick_rejects_source_and_descendants`, `move_to_same_folder_is_rejected_copy_is_allowed`, `opposite_pane_comes_first_and_is_default`, `duplicates_are_removed`, `summary_counts_files_and_folders`.
- [ ] **pick-dialog.slint:** 화면 사양대로. 왼쪽 트리는 Task 5의 `tree.rs`를 PICK 전용 인스턴스로 쓴다(거르기 입력이 있으면 이미 불러온 노드 중 이름이 맞는 것만 평평하게). 경로 입력은 Enter 때 작업 스레드에서 `stat_target`으로 폴더인지 확인 후 대상으로. 새 폴더 버튼은 이름 입력 창 → `Create` 작업 → 끝나면 그 폴더를 대상으로(PICK-03). 키보드: ↑↓ 대상 이동, Enter 확인, Esc 취소, 글자를 치면 거르기 입력으로(PICK-04).
- [ ] **연결:** F5·F6, 툴바 복사…·이동…, 오른쪽 클릭 메뉴가 PICK을 연다(선택이 없으면 커서 항목, 그것도 없으면 무시). 확인하면 `TransferSpec::into_dir(mode, &sources, &target, TransferOptions{conflict: 고른 정책, ..Default::default()})`로 제출하고 대상은 최근 목록 맨 앞에(최대 10, settings에 저장). 정책 매핑: 물어보기 Ask, 덮어쓰기 Overwrite, 건너뛰기 Skip, 이름 바꾸기 AutoRename.
- [ ] 스크린샷: 이동 PICK(원본 하위 폴더가 흐리게 표시된 상태). 검사·커밋: `feat(rf-ui): add the move and copy destination picker`.

### Task 8: 설정 저장, 테마 전환, 상태 복원

**Files:** `crates/rf-ui/src/settings.rs`, `controller.rs`, `ui/app.slint`

- [ ] **settings.rs:** `Settings { theme: ThemeChoice{System, Light, Dark}, show_hidden: bool, window: Option<(u32, u32)>, panes: [PaneSettings{tabs: Vec<String>, active: usize}; 2], active_pane: usize, recent_targets: Vec<String> }`(경로는 `VPath::to_uri()`), `load(config_dir) -> Settings`(없으면 기본값, 읽을 수 없거나 형식이 틀리면 `settings.toml.bad`로 옮기고 기본값), `save(&self, config_dir)`(같은 폴더의 `settings.toml.tmp`에 쓰고 `sync_file` 후 rename; 작업 스레드에서). `cargo add -p rf-ui toml serde --features serde/derive`. 테스트: `round_trips`, `corrupt_file_is_moved_aside`, `missing_folders_fall_back_to_home`(복원 시 없는 경로는 홈으로).
- [ ] **테마:** 툴바 오른쪽 "보기" 버튼 메뉴에 "밝게", "어둡게", "시스템 설정 따르기", "숨김 파일 보기"(체크). 시스템 설정은 Slint `Palette.color-scheme`을 읽어 `Theme.dark`에 묶는다.
- [ ] **복원:** 시작 때 창 크기, 패널별 탭과 활성 탭, 활성 패널, 숨김 표시를 복원. 저장은 바뀔 때마다가 아니라 0.5초 늦춘 한 번(`slint::Timer`)과 종료 때.
- [ ] 검사·커밋: `feat(rf-ui): remember tabs, theme and window size between runs`.

### Task 9: 앱 안 드래그 앤 드롭(이동 기본)

**Files:** `crates/rf-ui/ui/{file-list.slint,pane.slint,sidebar.slint}`, `controller.rs`

- [ ] Slint 1.18의 `DragArea`/`DropArea`(`builtin_elements.rs` 1779행 이후 설명과 `tests/syntax/elements/dragarea.slint`)로 목록 행을 끌어 반대 패널(빈 곳 = 그 패널 위치, 폴더 행 = 그 폴더), 탭, 트리 노드에 놓는다. `data-transfer`는 Rust 콜백에서 만든다. 끌기 시작한 패널의 선택 항목(커서 행이 선택 밖이면 그 행 하나)이 원본이다.
- [ ] 동작: 보조키 없음 = 이동(DND-01), Ctrl = 복사, Shift = 이동(Slint의 기본 규칙과 같음). 놓으면 PICK 없이 바로 `TransferSpec`(충돌 정책 Ask)을 제출하고 이동이면 끝난 뒤 알림 "항목 N개를 이동했습니다 [되돌리기]"(DND-07). 원본 폴더 자신·원본 하위 폴더·원래 있던 폴더 위에서는 `can-drop`이 `none`(DND-09). 놓을 수 있는 대상은 `has-drag`일 때 accent 테두리.
- [ ] Slint API가 이 동작을 지원하지 못하는 부분이 있으면(예: 데이터 전달 제한) 구현을 줄이고 보고서와 `docs/superpowers/plans/README.md` "계획 작성 중 확인된 사실"에 사실로 적는다. 꾸며 내지 않는다.
- [ ] 검사·커밋: `feat(rf-ui): move items by dragging them between panes, tabs and the tree`.

### Task 10: 데모 배포 산출물과 안내

**Files:** `.github/workflows/demo.yml`, `installer/macos/Info.plist`, `README.md`

- [ ] **demo.yml("Demo build"):** `push`(브랜치 `main`, `feat/**`), `workflow_dispatch`. 권한 `contents: read`. 매트릭스: `windows-latest`(`windows-x64`), `windows-11-arm`(`windows-arm64`), `macos-15`(`macos-arm64`), `ubuntu-latest`(`linux-x64`, Linux 패키지 설치 단계 포함). 단계: checkout, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `cargo build --release --locked -p racoon-finder`, 포장, `actions/upload-artifact@v4`(이름 `RacoonFinder-demo-<플랫폼>`, 보관 30일).
  - Windows: `racoon-finder.exe`, 빈 `portable` 표시 파일(데이터를 실행 파일 옆 `data` 폴더에만 남기고 폴더째 지우면 깨끗이 없어지도록, INST-06), `데모-안내.txt`를 zip으로.
  - macOS: `Racoon Finder.app/Contents/MacOS/racoon-finder`, `Contents/Info.plist`(`installer/macos/Info.plist`의 `__VERSION__`을 크레이트 버전으로 바꿈; `CFBundleIdentifier`는 `rf_core::identity::BUNDLE_ID` 값), `codesign --force --deep --sign - "Racoon Finder.app"`(애드혹 서명), `ditto -c -k --keepParent`로 zip.
  - Linux: 실행 파일과 안내를 tar.gz.
- [ ] **README.md "데모 실행":** GitHub Actions의 "Demo build" 실행 → Artifacts에서 내려받기. Windows: 압축 풀고 `racoon-finder.exe` 실행, SmartScreen 경고면 "추가 정보 → 실행"(서명 전 데모), 데이터는 옆의 `data` 폴더. macOS: 압축 풀고 앱을 오른쪽 클릭 → 열기(공증 전 데모), 막히면 `xattr -dr com.apple.quarantine "Racoon Finder.app"`. 데모가 할 수 있는 것(이 계획의 범위)과 아직 없는 것(위 "다루지 않는 항목" 요약). 실제 파일을 옮기고 지우므로 처음에는 연습용 폴더에서 써 보라는 주의.
- [ ] CI가 네 플랫폼 모두 녹색인지, 산출물이 올라왔는지 확인. 커밋: `ci: build downloadable demo packages for Windows, macOS and Linux`.

## 실행 순서와 병렬화

Task 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8 → 9 → 10 순서로 한다. Task 2와 Task 10의 워크플로 초안은 서로 파일이 겹치지 않아 병렬로 해도 되지만, Task 10의 최종 확인은 Task 9 뒤에 한다.
