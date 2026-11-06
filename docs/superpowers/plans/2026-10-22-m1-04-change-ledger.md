# M1-04 rf-ledger 변경 원장과 클린 삭제 코어 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 앱이 OS에 가하는 모든 변경(HKCU 레지스트리 값, macOS 기본값, 예약된 서비스 등록)을 바꾸기 전 값과 함께 선기록(write-ahead)으로 남기고 기능을 끄거나 앱을 지울 때 역순으로 되돌리는 `rf-ledger` 크레이트와, 원장 복원 뒤 앱 데이터 폴더를 지우는 클린 삭제 코어(dry-run 포함)를 만든다.

**Architecture:** 원장은 `AppPaths::data_dir()/ledger.json` 한 파일이고, 바꿀 때마다 같은 폴더의 임시 파일에 쓰고 fsync한 뒤 rename으로 원자적으로 교체한다. 변경은 `Change`(대상 슬롯 + 원하는 값)이고 실제 OS 접근은 `SystemBackend` 트레잇 뒤에 있다: Windows는 HKCU만 받는 `RegistryBackend`(windows-registry), macOS는 `PreferencesBackend`(CFPreferences), 테스트는 결함 주입이 되는 `FakeBackend`. 원장 로직·검증·바이트 인코딩은 순수 Rust라 Linux에서 모두 테스트하고, OS 백엔드는 CI의 Windows·macOS 러너가 실제 레지스트리·기본값으로 검증한다. 원장 밖에서 OS를 바꾸지 못하게 루트 `clippy.toml`의 `disallowed-methods`와 정책 테스트로 막는다.

**Tech Stack:** Rust stable(edition 2024, MSRV 1.85), rf-core(M1-01), serde·serde_json, thiserror, windows-registry(windows-rs 계열, Windows 전용), objc2-core-foundation(CFPreferences·CFPropertyList, macOS 전용), tempfile·toml(개발 의존성), clippy `disallowed-methods`.

**Spec:** docs/PRD.md

## Global Constraints

- M1-01 계획(`docs/superpowers/plans/2026-10-22-m1-01-workspace-core.md`)의 Global Constraints를 그대로 따른다: edition 2024, `rust-version = "1.85"`, `GPL-3.0-or-later`, 새 크레이트 매니페스트 상속 블록과 `[lints] workspace = true`, 의존성은 `cargo add`로만(버전 숫자를 손으로 쓰지 않음), `Cargo.lock` 커밋, 표준 검사(`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`)와 교차 검사(호스트가 아닌 두 타깃 `--target x86_64-pc-windows-msvc`·`aarch64-apple-darwin`·`x86_64-unknown-linux-gnu`로 같은 clippy), 영어 conventional commits, 커밋 전 `cargo fmt --all`.
- 제품 이름: `.rs` 파일 어디에도 제품 이름 철자를 쓰지 않는다. M1-01의 `product_name_appears_only_in_identity_module` 테스트가 `crates/`의 모든 `.rs`를 대소문자 무시로 검사한다. 테스트용 레지스트리 키와 기본값 도메인은 `rf_core::identity::{DATA_DIR_NAME, BUNDLE_ID}`로 조립한다(예: `format!(r"Software\{DATA_DIR_NAME}Test")` = PRD가 말하는 `HKCU\Software\RacoonFinderTest`). 문서 주석에서는 "the app"이라고 쓴다.
- INST-02: OS를 바꾸는 코드는 `rf_ledger::Ledger::apply`(기록)와 `revert_*`·`run_cleanup`(복원)만 거친다. 레지스트리·기본값 쓰기 API는 루트 `clippy.toml`의 `disallowed-methods`로 막혀 있고(Task 7), `clippy::disallowed_methods`를 억제해도 되는 곳은 `crates/rf-ledger` 안뿐이다. 제품 코드에서는 `src/backend/windows.rs`, `src/backend/macos.rs`의 모듈 첫 줄 `#![expect(clippy::disallowed_methods, …)]`만 쓴다(`expect`라서 `clippy.toml` 항목이 사라지면 오히려 경고가 난다).
- INST-03: rf-ledger가 만드는 파일은 `AppPaths::data_dir()/ledger.json`과, 원자적 교체를 위해 같은 폴더에 잠깐 생기는 `ledger.json.tmp` 둘뿐이다(rename은 같은 볼륨이어야 하므로 OS 임시 폴더가 아니라 소유 폴더 안에 둔다). 첫 변경을 기록하기 전에는 폴더도 만들지 않는다.
- WINX-06: 레지스트리 대상은 첫 구성요소가 `HKCU` 또는 `HKEY_CURRENT_USER`(ASCII 대소문자 무시)인 키만 받는다. `HKCR`은 HKCU에 키가 없으면 실제로 HKLM에 쓰이므로 다른 하이브와 함께 거부한다. 백엔드는 원장 파일에서 읽은 값도 매번 다시 검사한다(조작된 원장으로 HKLM을 건드리지 않게).
- INST-06: 포터블 모드(`AppPaths::mode() == InstallMode::Portable`)에서는 `SystemBackend`의 어떤 메서드도 부르지 않고 원장 파일을 읽지도 쓰지도 않는다.
- OS 코드: Windows 백엔드는 `#[cfg(target_os = "windows")]`, macOS 백엔드는 `#[cfg(target_os = "macos")]` 모듈이다. 그 밖의 OS에서 `os_backend()`는 모든 변경을 `LedgerError::Unsupported`로 거부하는 `UnsupportedBackend`를 준다. `unsafe`는 `src/backend/macos.rs`에만 있고(모듈 첫 줄 `#![allow(unsafe_code, reason = "…")]`, 블록마다 `// SAFETY:`), Windows 백엔드는 windows-registry의 안전한 API만 쓴다.
- 문서 주석의 식별자(`NSFileViewer`, `NSGlobalDomain`, `CFPreferences`, `HKCU`, `HKEY_CURRENT_USER`)는 백틱으로 감싼다(clippy pedantic `doc_markdown`).
- `Display` 문자열은 로그용 영어이고 이 계획에 적힌 그대로 쓴다. 테스트가 일부를 비교한다.
- 실제 OS를 건드리는 테스트의 범위: Windows는 `HKCU\Software\<DATA_DIR_NAME>Test`, `…Test-raw`, `…Test-refuse`만, macOS는 `<BUNDLE_ID>.ledgertest`, `<BUNDLE_ID>.ledgertest.clear` 도메인만 쓴다. 전역 `NSFileViewer`를 바꾸는 테스트는 환경 변수 `GITHUB_ACTIONS`가 있을 때만 돈다(개발자 Mac의 "Finder에서 보기"를 건드리지 않는다). 개발용 Mac에는 `~/Library/Preferences/<BUNDLE_ID>.ledgertest*.plist` 빈 파일이 남을 수 있다.
- Linux 컨테이너에서 실행할 때: `cfg(windows)`·`cfg(target_os = "macos")` 테스트는 0개로 컴파일되고, 실제 실행은 Task 10의 CI(`test (windows-latest)`, `test (windows-11-arm)`, `test (macos-15)`)가 맡는다. Windows·macOS 호스트에서는 각 태스크의 실행 명령을 그대로 돌린다. 이 계획에는 실기(물리 Windows 11·macOS 15)가 꼭 필요한 태스크가 없다.

## Review Focus

- 앱을 쓰는 동안 다른 앱(Directory Opus, Path Finder 등)이 같은 설정(폴더 열기 처리기, `NSFileViewer`)을 가져간 뒤 이 앱을 끄거나 지우는 경우 → 그 앱의 값을 덮어쓰지 않고 그대로 둔다. 되돌리기는 지금 값이 여전히 이 앱이 쓴 값일 때만 한다. (Task 3 `revert_leaves_values_someone_else_changed_since`)
- 클린 삭제 도중 강제 종료·전원 차단, 또는 일부 되돌리기 실패 → 원장과 데이터 폴더를 남기고, 다시 실행하면 남은 변경만 되돌린 뒤 끝까지 지운다. (Task 6 `cleanup_finishes_after_a_crash_mid_revert`, `cleanup_keeps_everything_when_a_revert_fails`)
- 원장 파일이 잘렸거나(디스크 오류) 더 새 버전 형식인 경우 → 열기를 거부하고 파일을 한 바이트도 바꾸지 않는다. 빈 원장으로 덮어쓰면 OS 변경이 영영 고아가 되므로, 클린 삭제도 아무것도 지우지 않는다. (Task 4 `corrupt_ledger_is_never_overwritten`, `newer_format_is_refused_untouched`, Task 6 `cleanup_refuses_to_delete_when_the_ledger_is_unreadable`)
- 사용자가 캐시·설정 폴더를 심볼릭 링크로 다른 드라이브에 옮겨 둔 경우 → 클린 삭제는 링크만 지우고 링크 대상의 내용은 남긴다. (Task 6 `cleanup_removes_links_but_not_what_they_point_to`)
- 설치형의 데이터 폴더(원장 포함)를 포터블 폴더의 `data`로 복사해 쓰는 경우 → 포터블 실행은 복사된 원장을 읽지도 되돌리지도 않아 설치형의 레지스트리를 건드리지 않는다. (Task 5 `portable_mode_ignores_a_copied_installed_ledger`)

---

## File Structure

```
clippy.toml                         (수정) disallowed-methods 항목 추가(Task 7)
crates/rf-ledger/
  Cargo.toml
  src/lib.rs                        모듈 선언과 재수출
  src/error.rs                      LedgerError
  src/change.rs                     Target·Value·Change·Before, 검증 규칙, HKCU 경로 검사, 레지스트리 바이트 인코딩(순수, 모든 OS)
  src/backend/mod.rs                SystemBackend 트레잇, UnsupportedBackend, os_backend()
  src/backend/fake.rs               FakeBackend: 메모리 슬롯, 호출 기록, 결함 주입
  src/backend/windows.rs            RegistryBackend: HKCU 전용(cfg windows)
  src/backend/macos.rs              PreferencesBackend: CFPreferences(cfg macos)
  src/store.rs                      원장 파일 읽기, 형식 버전 검사, 원자적 쓰기
  src/ledger.rs                     Ledger: 선기록 적용, 역순 복원, 시작 시 조정, 포터블 차단
  src/cleanup.rs                    plan_cleanup(dry-run), run_cleanup
  tests/common/mod.rs               tempdir AppPaths 픽스처, 슬롯·변경 도우미
  tests/change.rs  tests/ledger_apply.rs  tests/ledger_revert.rs  tests/ledger_recovery.rs
  tests/portable.rs  tests/cleanup.rs  tests/policy.rs  tests/windows_registry.rs  tests/macos_prefs.rs
```

## 후속 계획이 쓰는 공개 API (요약)

모든 항목은 `rf_ledger` 루트에서도 재수출된다(`rf_ledger::Ledger` 등). 세부 규칙은 각 태스크의 Interfaces에 있다.

```rust
// rf_ledger::change
pub const GLOBAL_DOMAIN: &str;   // "NSGlobalDomain"
pub const FILE_VIEWER_KEY: &str; // "NSFileViewer"
pub enum ServiceKind { WindowsService, LaunchAgent }
pub enum Target { Registry { key: String, name: String }, Preference { domain: String, key: String }, Service { service: ServiceKind, name: String } } // non_exhaustive, Display
pub enum Value { Str(String), ExpandStr(String), MultiStr(Vec<String>), U32(u32), U64(u64), Bytes(Vec<u8>), Raw { ty: u32, data: Vec<u8> }, Bool(bool), I64(i64), Plist(Vec<u8>), Program { path: String, args: Vec<String> } } // non_exhaustive
pub struct Change { pub target: Target, pub value: Option<Value> }
pub struct Before { pub value: Option<Value>, pub created_keys: Vec<String> }
impl Change { pub fn registry(key: &str, name: &str, value: Option<Value>) -> Result<Change, LedgerError>; pub fn preference(domain: &str, key: &str, value: Option<Value>) -> Result<Change, LedgerError>; pub fn service(service: ServiceKind, name: &str, value: Option<Value>) -> Result<Change, LedgerError>; pub fn macos_file_viewer() -> Change; pub fn validate(&self) -> Result<(), LedgerError> }
impl Target { pub fn same_slot(&self, other: &Target) -> bool }
pub fn hkcu_subkey(key: &str) -> Result<&str, LedgerError>;
pub fn registry_data(value: &Value) -> Result<(u32, Vec<u8>), LedgerError>;

// rf_ledger::backend
pub trait SystemBackend: Debug + Send { fn capture(&mut self, change: &Change) -> Result<Before, LedgerError>; fn apply(&mut self, change: &Change) -> Result<(), LedgerError>; fn restore(&mut self, change: &Change, before: &Before) -> Result<(), LedgerError>; fn is_applied(&mut self, change: &Change) -> Result<bool, LedgerError>; fn clear_domain(&mut self, domain: &str) -> Result<(), LedgerError> /* default Ok(()) */ }
pub struct UnsupportedBackend;
pub fn os_backend() -> Box<dyn SystemBackend>;
// rf_ledger::backend::fake
pub struct FakeBackend; pub enum FakeOp { Capture, Apply, Restore, IsApplied, ClearDomain }
impl FakeBackend { pub fn new() -> FakeBackend; pub fn set(&self, target: &Target, value: Option<Value>); pub fn get(&self, target: &Target) -> Option<Value>; pub fn calls(&self) -> Vec<FakeOp>; pub fn fail_next(&self, op: FakeOp, target: &Target); pub fn panic_after_next(&self, op: FakeOp, target: &Target) }
// rf_ledger::backend::windows (cfg windows): pub struct RegistryBackend;   rf_ledger::backend::macos (cfg macos): pub struct PreferencesBackend;

// rf_ledger::ledger
pub struct EntryId(pub u64); pub enum EntryState { Pending, Applied }
pub struct LedgerEntry { pub id: EntryId, pub group: String, pub state: EntryState, pub change: Change, pub before: Before } // non_exhaustive
pub enum ApplyOutcome { Recorded(EntryId), Unchanged }
pub struct EntryFailure { pub id: EntryId, pub change: String, pub error: String }
pub struct ReconcileReport { pub confirmed: Vec<EntryId>, pub rolled_back: Vec<EntryId>, pub failed: Vec<EntryFailure> }
pub struct RevertReport { pub reverted: Vec<EntryId>, pub skipped: Vec<EntryId>, pub failed: Vec<EntryFailure> } // is_clean()
impl Ledger {
    pub const FILE_NAME: &'static str; // "ledger.json"
    pub const FORMAT: u32;             // 1
    pub fn file_path(paths: &AppPaths) -> PathBuf;
    pub fn open(paths: &AppPaths, backend: Box<dyn SystemBackend>) -> Result<(Ledger, ReconcileReport), LedgerError>;
    pub fn inspect(paths: &AppPaths) -> Result<Vec<LedgerEntry>, LedgerError>;
    pub fn apply(&mut self, group: &str, change: Change) -> Result<ApplyOutcome, LedgerError>;
    pub fn entries(&self) -> &[LedgerEntry];
    pub fn revert_group(&mut self, group: &str) -> RevertReport;
    pub fn revert_all(&mut self) -> RevertReport;
}

// rf_ledger::cleanup
pub enum CleanupAction { Revert { id: EntryId, change: String }, ClearDomain(String), Delete(PathBuf) }
pub struct CleanupReport { pub reverted: RevertReport, pub done: Vec<CleanupAction>, pub failed: Vec<(CleanupAction, String)>, pub deletes_skipped: bool } // is_complete()
pub fn plan_cleanup(paths: &AppPaths) -> Result<Vec<CleanupAction>, LedgerError>;
pub fn run_cleanup(paths: &AppPaths, backend: Box<dyn SystemBackend>) -> Result<CleanupReport, LedgerError>;

// rf_ledger::error
pub enum LedgerError { PortableMode, OutsideHkcu(String), InvalidTarget(String), InvalidValue(String), InvalidGroup(String), SlotOwned { target: String, owner: String }, Unsupported(String), Backend(String), Corrupt { path: PathBuf, reason: String }, UnsupportedFormat { path: PathBuf, found: u32, supported: u32 }, Core(rf_core::Error) } // non_exhaustive
```

후속 계획의 사용 규칙: 기능마다 그룹 이름(소문자·숫자·`.`·`-`, 예: `winx.explorer`, `macx.file-viewer`)을 하나 정해 `apply`하고, 기능을 끌 때 `revert_group`을 부른다. 같은 슬롯을 두 그룹이 나눠 쓸 수 없다(`SlotOwned`). 앱은 시작할 때 `Ledger::open(&paths, os_backend())`를 한 번 하고 그 `Ledger`를 공유한다. 다른 크레이트의 테스트는 `FakeBackend`로 원장을 연다.

## 이 계획이 일부러 다루지 않는 PRD 항목

- INST-01 설치 프로그램과 설치 옵션, INST-04 (1) 프로세스 종료·(5) 설치 폴더와 제거 프로그램 등록 삭제·"삭제 전에 설정 백업" 체크박스: 설치 프로그램 계획. 제거 프로그램은 `run_cleanup`을 부른다.
- INST-04 (3) 자격 증명 삭제: REM-11 계획이 자격 증명 항목을 원장 변경 종류로 더해 (2) 단계에서 함께 되돌린다.
- INST-04 (4) 점프 목록: WINX-10 계획.
- INST-05의 앱 번들 휴지통 이동, "완전 삭제…" 메뉴, 첫 실행 안내, Homebrew cask `zap`: MACX(M2)·배포 계획. 이 계획은 그 메뉴가 부르는 `run_cleanup`까지만 만든다.
- INST-07 VM 스냅샷 비교 CI, INST-08 설정 마이그레이션, INST-09 패키지 관리자: 각 계획.
- INST-03의 "종료할 때 임시 파일 삭제": 메인 앱 수명 주기 계획. 이 계획은 삭제 때 `TEMP_PREFIX` 항목을 지우는 것까지만 한다.
- WINX-01~05, 07~10: WINX 계획이 실제 HKCU 키 목록(M0 결과의 등록 변형)을 정해 `Change::registry`로 기록한다. 파일 연결·시작 프로그램(`Run` 키)·알림 등록(`AppUserModelId`)도 HKCU 레지스트리 종류로 표현된다.
- MACX-01의 설정 UI와 앱 호환성 표, MACX-06 로그인 항목: MACX(M2) 계획. 이 계획은 `Change::macos_file_viewer()`와 그 백엔드까지만 한다.
- IDX-05 서비스 등록의 백엔드(관리자 승인, Windows 서비스 생성·삭제, `SMAppService` LaunchAgent): M3 IDX 계획. 이 계획은 `Target::Service`·`Value::Program` 기록 형식만 예약하고 모든 실제 백엔드는 `Unsupported`를 돌려준다.
- 프로세스 간 원장 잠금: 단일 인스턴스(WINX-03 IPC) 계획과 INST-04 (1) 단계가 동시 실행을 막는다.
- macOS가 번들 ID로 자동으로 만드는 위치(`Saved Application State`, `HTTPStorages` 등): INST-07 VM 비교 결과로 rf-core `AppPaths::owned_paths()`를 보강한다(rf-core 변경).

---

### Task 1: rf-ledger 골격과 변경 모델

**Files:**
- Create: `crates/rf-ledger/Cargo.toml`, `crates/rf-ledger/src/lib.rs`, `crates/rf-ledger/src/error.rs`, `crates/rf-ledger/src/change.rs`
- Test: `crates/rf-ledger/tests/change.rs`
- 수정됨: `Cargo.lock`

**Interfaces:**
- Consumes: `rf_core::identity::BUNDLE_ID`, `rf_core::Error` (M1-01)
- Produces: `rf_ledger::error::LedgerError`, `rf_ledger::change::{GLOBAL_DOMAIN, FILE_VIEWER_KEY, ServiceKind, Target, Value, Change, Before, hkcu_subkey, registry_data}`와 위 요약의 메서드. 재수출 `rf_ledger::{Before, Change, LedgerError, ServiceKind, Target, Value}`. JSON 형태(원장 파일 형식)는 아래 `json_shape_is_stable` 테스트로 고정된다.

- [ ] **Step 1: 크레이트를 만들고 의존성을 추가한다**

`crates/rf-ledger/Cargo.toml`: M1-01 Task 1의 `rf-core` 매니페스트와 같은 상속 블록(`version.workspace = true` … `publish.workspace = true`)과 `[lints] workspace = true`, `name = "rf-ledger"`, `description = "System change ledger: records every OS change with its before-state and reverts them for a clean uninstall."`, 빈 `[dependencies]`. `src/lib.rs`는 `//! System change ledger (PRD INST-02): every OS change the app makes, with its before-state.` 한 줄.

Run: `cargo add -p rf-ledger --path crates/rf-core` 그리고 `cargo add -p rf-ledger serde -F derive` 그리고 `cargo add -p rf-ledger serde_json thiserror`
Expected: `Adding rf-core (local)`, `Adding serde v1.`, `Adding serde_json v1.`, `Adding thiserror v2.` 로 시작하는 줄

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/change.rs`:

```rust
use rf_core::identity::BUNDLE_ID;
use rf_ledger::change::{FILE_VIEWER_KEY, GLOBAL_DOMAIN, hkcu_subkey, registry_data};
use rf_ledger::{Change, LedgerError, ServiceKind, Target, Value};

fn reg_err(key: &str) -> Option<LedgerError> {
    Change::registry(key, "v", None).err()
}

#[test]
fn registry_changes_accept_only_hkcu() {
    let c = Change::registry(r"HKEY_CURRENT_USER\Software\X", "", None).unwrap();
    assert_eq!(c.target, Target::Registry { key: r"HKCU\Software\X".into(), name: String::new() });
    assert_eq!(hkcu_subkey(r"hkcu\Software\X").unwrap(), r"Software\X");
    for bad in [
        r"HKLM\Software\X", r"HKEY_LOCAL_MACHINE\Software\X", r"HKCR\.txt", r"HKEY_CLASSES_ROOT\.txt",
        r"HKU\S-1-5-18\X", r"HKEY_USERS\X", r"HKCC\X", r"Software\X", "HKCU/Software/X", r"HKCUX\Software",
    ] {
        assert!(matches!(reg_err(bad), Some(LedgerError::OutsideHkcu(k)) if k == bad), "{bad}");
    }
}

#[test]
fn registry_changes_reject_malformed_keys_names_and_values() {
    let long = format!(r"HKCU\{}", "k".repeat(256));
    for bad in ["HKCU", r"HKCU\", r"HKCU\a\\b", r"HKCU\a\", "HKCU\\a\0b", long.as_str()] {
        assert!(matches!(reg_err(bad), Some(LedgerError::InvalidTarget(_))), "{bad:?}");
    }
    assert!(matches!(Change::registry(r"HKCU\a", "n\0", None), Err(LedgerError::InvalidTarget(_))));
    for v in [
        Value::Str("a\0b".into()),
        Value::MultiStr(vec!["a".into(), String::new()]),
        Value::Bool(true),
        Value::Program { path: "x".into(), args: vec![] },
    ] {
        assert!(matches!(Change::registry(r"HKCU\a", "n", Some(v.clone())), Err(LedgerError::InvalidValue(_))), "{v:?}");
    }
}

#[test]
fn preference_changes_reject_paths_and_registry_values() {
    assert!(Change::preference(GLOBAL_DOMAIN, FILE_VIEWER_KEY, Some(Value::Bool(true))).is_ok());
    for (domain, key) in [("/tmp/x.plist", "k"), ("~/Library/x", "k"), ("", "k"), ("a.b", ""), ("a\0b", "k"), (".hidden", "k")] {
        assert!(matches!(Change::preference(domain, key, None), Err(LedgerError::InvalidTarget(_))), "{domain:?} {key:?}");
    }
    for v in [Value::U32(1), Value::Raw { ty: 1, data: vec![] }, Value::ExpandStr("x".into())] {
        assert!(matches!(Change::preference("a.b", "k", Some(v)), Err(LedgerError::InvalidValue(_))));
    }
}

#[test]
fn file_viewer_change_points_at_the_app() {
    assert_eq!((GLOBAL_DOMAIN, FILE_VIEWER_KEY), ("NSGlobalDomain", "NSFileViewer"));
    let c = Change::macos_file_viewer();
    assert_eq!(c.target, Target::Preference { domain: GLOBAL_DOMAIN.into(), key: FILE_VIEWER_KEY.into() });
    assert_eq!(c.value, Some(Value::Str(BUNDLE_ID.into())));
    assert!(c.validate().is_ok());
}

#[test]
fn service_change_is_a_reserved_record_shape() {
    let program = Value::Program { path: r"C:\x\rf-indexd.exe".into(), args: vec!["--service".into()] };
    let c = Change::service(ServiceKind::WindowsService, "rf-indexd", Some(program)).unwrap();
    let json = serde_json::to_string(&c).unwrap();
    assert_eq!(serde_json::from_str::<Change>(&json).unwrap(), c);
    assert!(matches!(Change::service(ServiceKind::LaunchAgent, "a/b", None), Err(LedgerError::InvalidTarget(_))));
    assert!(matches!(
        Change::service(ServiceKind::LaunchAgent, "a", Some(Value::Str("x".into()))),
        Err(LedgerError::InvalidValue(_))
    ));
}

#[test]
fn json_shape_is_stable() {
    let c = Change::registry(r"HKCU\Software\X", "", Some(Value::Str("a".into()))).unwrap();
    assert_eq!(
        serde_json::to_string(&c).unwrap(),
        r#"{"target":{"kind":"registry","key":"HKCU\\Software\\X","name":""},"value":{"type":"str","data":"a"}}"#
    );
    let raw = Value::Raw { ty: 0, data: vec![1, 2] };
    assert_eq!(serde_json::to_string(&raw).unwrap(), r#"{"type":"raw","data":{"ty":0,"data":[1,2]}}"#);
    for v in [
        Value::ExpandStr("%A%".into()), Value::MultiStr(vec!["a".into()]), Value::U32(1), Value::U64(2),
        Value::Bytes(vec![3]), Value::Bool(true), Value::I64(-1), Value::Plist(vec![4]), raw,
    ] {
        let back: Value = serde_json::from_str(&serde_json::to_string(&v).unwrap()).unwrap();
        assert_eq!(back, v);
    }
}

#[test]
fn registry_slots_compare_case_insensitively() {
    let a = Target::Registry { key: r"HKCU\Software\X".into(), name: "V".into() };
    assert!(a.same_slot(&Target::Registry { key: r"hkcu\software\x".into(), name: "v".into() }));
    assert!(!a.same_slot(&Target::Registry { key: r"HKCU\Software\X".into(), name: "W".into() }));
    let p = Target::Preference { domain: "A".into(), key: "k".into() };
    assert!(!p.same_slot(&Target::Preference { domain: "a".into(), key: "k".into() }));
}

#[test]
fn registry_data_encoding() {
    assert_eq!(registry_data(&Value::Str("ab".into())).unwrap(), (1, vec![0x61, 0, 0x62, 0, 0, 0]));
    assert_eq!(registry_data(&Value::ExpandStr("a".into())).unwrap(), (2, vec![0x61, 0, 0, 0]));
    assert_eq!(
        registry_data(&Value::MultiStr(vec!["a".into(), "b".into()])).unwrap(),
        (7, vec![0x61, 0, 0, 0, 0x62, 0, 0, 0, 0, 0])
    );
    assert_eq!(registry_data(&Value::MultiStr(vec![])).unwrap(), (7, vec![0, 0]));
    assert_eq!(registry_data(&Value::U32(0x0102_0304)).unwrap(), (4, vec![4, 3, 2, 1]));
    assert_eq!(registry_data(&Value::U64(1)).unwrap(), (11, vec![1, 0, 0, 0, 0, 0, 0, 0]));
    assert_eq!(registry_data(&Value::Bytes(vec![9])).unwrap(), (3, vec![9]));
    assert_eq!(registry_data(&Value::Raw { ty: 0, data: vec![1, 2] }).unwrap(), (0, vec![1, 2]));
    assert!(matches!(registry_data(&Value::Bool(true)), Err(LedgerError::InvalidValue(_))));
}

#[test]
fn targets_display_for_logs_and_dry_runs() {
    let reg = |name: &str| Target::Registry { key: r"HKCU\Software\X".into(), name: name.into() };
    assert_eq!(reg("").to_string(), r"HKCU\Software\X @ (default)");
    assert_eq!(reg("V").to_string(), r"HKCU\Software\X @ V");
    assert_eq!(Change::macos_file_viewer().target.to_string(), "defaults NSGlobalDomain NSFileViewer");
    assert_eq!(Target::Service { service: ServiceKind::LaunchAgent, name: "a".into() }.to_string(), "launch_agent a");
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test -p rf-ledger --test change`
Expected: FAIL, `error[E0432]: unresolved import` (`rf_ledger::change`, `rf_ledger::Change` 등)

- [ ] **Step 4: `error.rs`와 `change.rs`를 구현하고 lib.rs에 `pub mod error; pub mod change;`와 재수출을 추가한다**

`error.rs` (메시지 고정):

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LedgerError {
    #[error("portable mode never changes the operating system")]
    PortableMode,
    #[error("registry key is outside HKEY_CURRENT_USER: {0}")]
    OutsideHkcu(String),
    #[error("invalid change target: {0}")]
    InvalidTarget(String),
    #[error("invalid value: {0}")]
    InvalidValue(String),
    #[error("invalid change group `{0}`: use lowercase letters, digits, '.' and '-'")]
    InvalidGroup(String),
    #[error("{target} already belongs to change group `{owner}`")]
    SlotOwned { target: String, owner: String },
    #[error("not supported here: {0}")]
    Unsupported(String),
    #[error("system backend failed: {0}")]
    Backend(String),
    #[error("ledger file {} is corrupt: {reason}", path.display())]
    Corrupt { path: PathBuf, reason: String },
    #[error("ledger file {} has format {found}; this build reads up to {supported}", path.display())]
    UnsupportedFormat { path: PathBuf, found: u32, supported: u32 },
    #[error(transparent)]
    Core(#[from] rf_core::Error),
}
```

`change.rs` 타입 정의(직렬화 속성이 원장 파일 형식을 정하므로 그대로 쓴다):

```rust
pub const GLOBAL_DOMAIN: &str = "NSGlobalDomain";
pub const FILE_VIEWER_KEY: &str = "NSFileViewer";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceKind { WindowsService, LaunchAgent }

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Target {
    Registry { key: String, name: String },
    Preference { domain: String, key: String },
    Service { service: ServiceKind, name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[non_exhaustive]
pub enum Value {
    Str(String),
    ExpandStr(String),
    MultiStr(Vec<String>),
    U32(u32),
    U64(u64),
    Bytes(Vec<u8>),
    Raw { ty: u32, data: Vec<u8> },
    Bool(bool),
    I64(i64),
    Plist(Vec<u8>),
    Program { path: String, args: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub target: Target,
    pub value: Option<Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Before {
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created_keys: Vec<String>,
}
```

문서 주석으로 남길 의미: `Registry.key`는 정규형 `HKCU\<subkey>`, `name`이 빈 문자열이면 기본값. `Preference.domain`은 `NSGlobalDomain` 또는 번들 ID이고 범위는 현재 사용자·모든 호스트(`defaults`의 기본 범위). `Service`와 `Program`은 IDX-05용 예약 형식이며 아직 어떤 실제 백엔드도 처리하지 않는다. `Value::None`이 아니라 `Change.value == None`이 "없애기"다. `Raw`·`Plist`는 백엔드가 바꾸기 전 값을 손실 없이 담는 형식이다. `Before.created_keys`는 `apply`가 새로 만들 레지스트리 키(정규형, 얕은 것부터).

규칙(테스트가 정하지 않는 부분):
- `hkcu_subkey(key)`: 첫 `\`에서 나눈다(없으면 전체가 첫 구성요소). 첫 구성요소가 `HKCU`·`HKEY_CURRENT_USER`(ASCII 대소문자 무시)가 아니면 `OutsideHkcu(key)`. 나머지가 비었거나, `\`로 나눈 구성요소 중 빈 것·NUL이 든 것·UTF-16 단위 255개 초과가 있으면 `InvalidTarget(key)`. 나머지를 돌려준다.
- `Change::registry`: `hkcu_subkey`의 결과로 `key`를 `format!("HKCU\\{subkey}")`로 정규화해 만든 뒤 `validate()`. `preference`·`service`도 만든 뒤 `validate()`.
- `validate()`: Registry는 `hkcu_subkey(key)`, 값 이름에 NUL이 있거나 UTF-16 단위 16,383개 초과면 `InvalidTarget(name)`, 값은 `Str`/`ExpandStr`(NUL 없음), `MultiStr`(빈 원소·NUL 없음), `U32`, `U64`, `Bytes`, `Raw`만. Preference는 domain이 비었거나 `/`·NUL을 포함하거나 `.`·`~`로 시작하면 `InvalidTarget(domain)`(CFPreferences는 경로를 도메인으로 받으면 그 파일에 쓴다), key가 비었거나 NUL을 포함하면 `InvalidTarget(key)`, 값은 `Str`, `Bool`, `I64`, `Bytes`, `Plist`만. Service는 name이 비었거나 `/`·`\`·NUL을 포함하면 `InvalidTarget(name)`, 값은 `Program`(path가 비어 있지 않음)만. 허용되지 않은 값은 `InvalidValue(format!("{value:?} is not valid for {target}"))`.
- `same_slot`: 같은 변형일 때만 `true`. Registry는 key와 name을 각각 `to_lowercase()`로 비교(레지스트리는 대소문자를 구분하지 않는다), 나머지는 정확히 같아야 한다.
- `Display for Target`: Registry `"{key} @ {name}"`(빈 name은 `(default)`), Preference `"defaults {domain} {key}"`, Service `"{service} {name}"`(serde 이름 `windows_service`/`launch_agent`).
- `registry_data`: `Str` 1, `ExpandStr` 2, `Bytes` 3, `U32` 4(LE), `MultiStr` 7, `U64` 11(LE), `Raw`는 그대로. 문자열은 UTF-16LE에 NUL 단위 하나, `MultiStr`은 원소마다 NUL을 붙이고 끝에 NUL 하나 더(빈 목록은 NUL 하나). 그 밖의 값은 `InvalidValue`. windows-registry의 `set_string`·`set_multi_string`과 같은 바이트다.
- `macos_file_viewer()`: `Preference { GLOBAL_DOMAIN, FILE_VIEWER_KEY }`에 `Value::Str(rf_core::identity::BUNDLE_ID)`.

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test -p rf-ledger --test change`
Expected: `test result: ok. 9 passed`

- [ ] **Step 6: 표준 검사를 돌린다**

- [ ] **Step 7: 커밋한다**

```bash
git add Cargo.lock crates/rf-ledger
git commit -m "feat(rf-ledger): add change model with HKCU-only registry targets"
```

---

### Task 2: 백엔드 트레잇, FakeBackend, 선기록 적용

**Files:**
- Create: `crates/rf-ledger/src/backend/mod.rs`, `crates/rf-ledger/src/backend/fake.rs`, `crates/rf-ledger/src/store.rs`, `crates/rf-ledger/src/ledger.rs`
- Modify: `crates/rf-ledger/src/lib.rs`, `crates/rf-ledger/Cargo.toml` (cargo add)
- Test: `crates/rf-ledger/tests/common/mod.rs`, `crates/rf-ledger/tests/ledger_apply.rs`

**Interfaces:**
- Consumes: Task 1의 `Change`, `Target`, `Value`, `Before`, `LedgerError`, `Change::validate`, `Target::same_slot`; rf-core `AppPaths::{data_dir, mode}`, `InstallMode`, `Error::io`, `VPath::local`
- Produces:
  - `rf_ledger::backend::SystemBackend` (아래 정의 그대로), `pub struct UnsupportedBackend;`(`Clone, Copy, Debug, Default`), `pub fn os_backend() -> Box<dyn SystemBackend>`(이 태스크에서는 모든 OS에서 `UnsupportedBackend`, Task 8·9가 Windows·macOS 갈래를 바꾼다)
  - `rf_ledger::backend::fake::{FakeBackend, FakeOp}`: 요약 절의 시그니처. `FakeBackend`는 `Clone, Debug, Default`이고 복제본끼리 상태를 공유한다(`Arc<Mutex<…>>`), 그래서 테스트가 복제본 하나를 원장에 넘기고 다른 복제본으로 살펴본다.
  - `rf_ledger::ledger::{EntryId, EntryState, LedgerEntry, ApplyOutcome, EntryFailure, ReconcileReport, Ledger}`와 `Ledger::{FILE_NAME, FORMAT, file_path, open, inspect, apply, entries}`(요약 절의 시그니처). `Ledger`는 `Debug + Send`.
  - 재수출: `rf_ledger::{SystemBackend, UnsupportedBackend, os_backend, FakeBackend, FakeOp, EntryId, EntryState, LedgerEntry, ApplyOutcome, EntryFailure, ReconcileReport, Ledger}`

- [ ] **Step 1: 개발 의존성을 추가한다**

Run: `cargo add -p rf-ledger --dev tempfile`
Expected: `Adding tempfile v3.` 로 시작하는 줄

- [ ] **Step 2: 테스트 공용 픽스처를 만든다**

`crates/rf-ledger/tests/common/mod.rs`:

```rust
#![allow(
    dead_code,
    clippy::unnecessary_wraps,
    reason = "each test binary uses a different subset; `s` mirrors `Change::value`"
)]

use std::path::{Path, PathBuf};

use rf_core::{AppPaths, BaseDirs, Os};
use rf_ledger::{Change, FakeBackend, Ledger, Target, Value};

pub struct Fx {
    pub dir: tempfile::TempDir,
    pub paths: AppPaths,
    pub fake: FakeBackend,
}

impl Fx {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn open(&self) -> Ledger {
        Ledger::open(&self.paths, Box::new(self.fake.clone())).unwrap().0
    }
}

fn fixture(os: Os, portable: bool) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("bin")).unwrap();
    std::fs::create_dir_all(root.join("Temp")).unwrap();
    if portable {
        std::fs::write(root.join("bin").join(rf_core::identity::PORTABLE_MARKER), b"").unwrap();
    }
    let base = BaseDirs {
        home: Some(root.join("home")),
        app_data: Some(root.join("Roaming")),
        local_app_data: Some(root.join("Local")),
        temp: root.join("Temp"),
    };
    let paths = AppPaths::resolve(os, &root.join("bin").join("app.exe"), &base).unwrap();
    Fx { dir, paths, fake: FakeBackend::new() }
}

/// Windows installed layout under a tempdir (pure path logic, works on every host).
pub fn installed() -> Fx {
    fixture(Os::Windows, false)
}

pub fn portable() -> Fx {
    fixture(Os::Windows, true)
}

pub fn macos() -> Fx {
    fixture(Os::MacOs, false)
}

/// Registry slot `HKCU\Software\T<n>`, value name `v`.
pub fn slot(n: u32) -> Target {
    Target::Registry { key: format!(r"HKCU\Software\T{n}"), name: "v".into() }
}

pub fn set(target: &Target, v: &str) -> Change {
    Change { target: target.clone(), value: s(v) }
}

pub fn s(v: &str) -> Option<Value> {
    Some(Value::Str(v.into()))
}

/// Every file (not directory) below `dir`, sorted.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() { stack.push(p) } else { out.push(p) }
        }
    }
    out.sort();
    out
}
```

- [ ] **Step 3: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/ledger_apply.rs`:

```rust
mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::*;
use rf_ledger::{ApplyOutcome, Change, EntryId, EntryState, FakeOp, Ledger, LedgerError, Target};

#[test]
fn apply_captures_before_state_then_changes_the_slot() {
    fn assert_send<T: Send>() {}
    assert_send::<Ledger>();
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("explorer"));
    let mut l = fx.open();
    assert_eq!(l.apply("winx.test", set(&t, "ours")).unwrap(), ApplyOutcome::Recorded(EntryId(1)));
    assert_eq!(fx.fake.get(&t), s("ours"));
    assert_eq!(fx.fake.calls(), [FakeOp::IsApplied, FakeOp::Capture, FakeOp::Apply]);
    let e = &l.entries()[0];
    assert_eq!((e.id, e.group.as_str(), e.state), (EntryId(1), "winx.test", EntryState::Applied));
    assert_eq!((e.before.value.clone(), e.change.clone()), (s("explorer"), set(&t, "ours")));
}

#[test]
fn crash_during_apply_leaves_a_pending_record() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    fx.fake.panic_after_next(FakeOp::Apply, &t);
    assert!(catch_unwind(AssertUnwindSafe(|| l.apply("g", set(&t, "ours")))).is_err());
    let on_disk = Ledger::inspect(&fx.paths).unwrap();
    assert_eq!(on_disk.len(), 1);
    assert_eq!((on_disk[0].state, on_disk[0].before.value.clone()), (EntryState::Pending, None));
    assert_eq!(fx.fake.get(&t), s("ours"));
}

#[test]
fn reapplying_the_live_value_records_nothing() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    l.apply("g", set(&t, "ours")).unwrap();
    let before = fx.fake.calls().len();
    assert_eq!(l.apply("g", set(&t, "ours")).unwrap(), ApplyOutcome::Unchanged);
    assert_eq!(fx.fake.calls()[before..], [FakeOp::IsApplied]);
    assert_eq!(l.entries().len(), 1);
}

#[test]
fn a_second_change_to_a_slot_records_the_previous_value() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    l.apply("g", set(&t, "a")).unwrap();
    assert_eq!(l.apply("g", set(&t, "b")).unwrap(), ApplyOutcome::Recorded(EntryId(2)));
    assert_eq!(l.entries()[1].before.value, s("a"));
}

#[test]
fn another_group_cannot_take_an_owned_slot() {
    let fx = installed();
    let mut l = fx.open();
    l.apply("winx.a", set(&slot(1), "a")).unwrap();
    let same = Target::Registry { key: r"hkcu\software\t1".into(), name: "V".into() };
    let err = l.apply("winx.b", set(&same, "b")).unwrap_err();
    assert!(matches!(err, LedgerError::SlotOwned { ref owner, .. } if owner == "winx.a"), "{err:?}");
    assert_eq!(fx.fake.get(&slot(1)), s("a"));
}

#[test]
fn failed_apply_is_rolled_back_and_not_recorded() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &t);
    assert!(matches!(l.apply("g", set(&t, "ours")), Err(LedgerError::Backend(_))));
    assert!(l.entries().is_empty() && Ledger::inspect(&fx.paths).unwrap().is_empty());
    assert_eq!(fx.fake.get(&t), s("orig"));
}

#[test]
fn failed_apply_with_failed_rollback_stays_pending() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &t);
    fx.fake.fail_next(FakeOp::Restore, &t);
    assert!(l.apply("g", set(&t, "ours")).is_err());
    assert_eq!(l.entries()[0].state, EntryState::Pending);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap()[0].state, EntryState::Pending);
}

#[test]
fn capture_failure_writes_nothing() {
    let fx = installed();
    let t = slot(1);
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Capture, &t);
    assert!(l.apply("g", set(&t, "ours")).is_err());
    assert!(!fx.paths.data_dir().exists());
}

#[test]
fn invalid_changes_and_groups_never_reach_the_backend() {
    let fx = installed();
    let mut l = fx.open();
    let hklm = Change { target: Target::Registry { key: r"HKLM\Software\X".into(), name: String::new() }, value: None };
    assert!(matches!(l.apply("g", hklm), Err(LedgerError::OutsideHkcu(_))));
    for group in ["", "WinX", "a b"] {
        assert!(matches!(l.apply(group, set(&slot(1), "x")), Err(LedgerError::InvalidGroup(_))), "{group:?}");
    }
    assert!(fx.fake.calls().is_empty());
}

#[test]
fn ledger_writes_only_its_own_file() {
    let fx = installed();
    let mut l = fx.open();
    assert!(!fx.paths.data_dir().exists());
    l.apply("g", set(&slot(1), "x")).unwrap();
    assert_eq!(Ledger::file_path(&fx.paths), fx.paths.data_dir().join("ledger.json"));
    assert_eq!(files_under(fx.root()), [Ledger::file_path(&fx.paths)]);
}

#[test]
fn entries_survive_reopening() {
    let fx = installed();
    let mut l = fx.open();
    l.apply("g", set(&slot(1), "a")).unwrap();
    l.apply("g", set(&slot(2), "b")).unwrap();
    let saved = l.entries().to_vec();
    drop(l);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap(), saved);
    let mut again = fx.open();
    assert_eq!(again.entries(), saved.as_slice());
    assert_eq!(again.apply("g", set(&slot(3), "c")).unwrap(), ApplyOutcome::Recorded(EntryId(3)));
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
#[test]
fn other_os_backend_refuses_every_change() {
    let fx = installed();
    let (mut l, _) = Ledger::open(&fx.paths, rf_ledger::os_backend()).unwrap();
    assert!(matches!(l.apply("g", set(&slot(1), "x")), Err(LedgerError::Unsupported(_))));
    assert!(!fx.paths.data_dir().exists());
}
```

- [ ] **Step 4: 실패를 확인한다**

Run: `cargo test -p rf-ledger --test ledger_apply`
Expected: FAIL, `error[E0432]: unresolved imports` (`rf_ledger::ApplyOutcome`, `rf_ledger::FakeBackend`, `rf_ledger::Ledger` 등)

- [ ] **Step 5: 백엔드 트레잇과 가짜를 구현한다**

`backend/mod.rs`(트레잇은 그대로 쓴다):

```rust
/// The only way the app touches OS settings. Every method must be safe to repeat.
pub trait SystemBackend: std::fmt::Debug + Send {
    /// Reads the live state of `change.target` as the before-state to record
    /// (for a registry value with `Some` value, also the keys `apply` would create).
    fn capture(&mut self, change: &Change) -> Result<Before, LedgerError>;
    /// Makes the live slot equal `change.value`; `None` removes it.
    fn apply(&mut self, change: &Change) -> Result<(), LedgerError>;
    /// Puts the slot back to `before` (succeeds when it already matches) and removes
    /// `before.created_keys` that are empty.
    fn restore(&mut self, change: &Change, before: &Before) -> Result<(), LedgerError>;
    /// Whether the live slot equals `change.value` right now.
    fn is_applied(&mut self, change: &Change) -> Result<bool, LedgerError>;
    /// Removes every key of a preferences domain the app owns (INST-05). Default: nothing to do.
    fn clear_domain(&mut self, _domain: &str) -> Result<(), LedgerError> {
        Ok(())
    }
}
```

`UnsupportedBackend`: 네 메서드 모두 `Err(LedgerError::Unsupported("this OS has no system backend".into()))`, `clear_domain`은 기본 구현. `os_backend()`는 `Box::new(UnsupportedBackend)`. lib.rs에 `pub mod backend;`(그 안에 `pub mod fake;`).

`backend/fake.rs`: 상태는 `live: BTreeMap<Target, Value>`, `calls: Vec<FakeOp>`, `faults: Vec<(FakeOp, Target, Fault)>`(비공개 `enum Fault { Fail, PanicAfter }`). 의미:
- 트레잇 호출은 결과와 상관없이 먼저 `calls`에 기록한다. `clear_domain`은 `FakeOp::ClearDomain`을 기록하고 `Ok(())`.
- 그 호출의 `(op, change.target)`과 같은 결함이 있으면 하나를 꺼낸다. `Fail`이면 효과 없이 `Err(LedgerError::Backend("injected failure".into()))`. `PanicAfter`면 효과를 낸 뒤 잠금을 풀고 `panic!("injected crash")`(프로세스가 죽은 상황의 흉내). 결함은 한 번 쓰면 사라진다.
- 효과: `capture` → `Before { value: live.get(target).cloned(), created_keys: vec![] }`, `apply` → `change.value`로 슬롯을 덮거나 지움, `restore` → `before.value`로 덮거나 지움, `is_applied` → `live.get(target) == change.value.as_ref()`. 슬롯은 `Target`이 정확히 같을 때만 같다(대소문자 규칙은 원장 쪽 `same_slot`이 맡는다). 어떤 `Target` 종류든 받는다(`Service` 포함, 후속 계획이 백엔드 없이 원장 로직을 테스트하도록).
- `set`·`get`은 기록하지 않는 직접 조작(다른 앱이나 사용자가 값을 바꾼 상황). 잠금이 독살(poison)돼도 `unwrap_or_else(PoisonError::into_inner)`로 계속 쓴다.

- [ ] **Step 6: 저장소와 `Ledger`를 구현한다**

`ledger.rs` 공개 타입:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryState { Pending, Applied }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct LedgerEntry {
    pub id: EntryId,
    pub group: String,
    pub state: EntryState,
    pub change: Change,
    pub before: Before,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome { Recorded(EntryId), Unchanged }

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EntryFailure { pub id: EntryId, pub change: String, pub error: String }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReconcileReport { pub confirmed: Vec<EntryId>, pub rolled_back: Vec<EntryId>, pub failed: Vec<EntryFailure> }
```

`EntryFailure.change`는 `entry.change.target.to_string()`, `error`는 `LedgerError::to_string()`. `Ledger`의 비공개 필드: 파일 경로, `portable: bool`(`paths.mode() == InstallMode::Portable`), `backend: Box<dyn SystemBackend>`, `next_id: u64`, `entries: Vec<LedgerEntry>`(id 오름차순). `#[derive(Debug)]`.

`store.rs`(크레이트 내부): `pub(crate) struct LedgerFile { format: u32, next_id: u64, entries: Vec<LedgerEntry> }`(serde, 필드 이름 그대로).
- `pub(crate) fn load(path: &Path) -> Result<Option<LedgerFile>, LedgerError>`: 파일이 없으면 `Ok(None)`. 읽기 오류는 `LedgerError::Core(rf_core::Error::io("read", …))`. JSON 오류는 `Corrupt { path, reason: serde 오류 문자열 }`. 형식 번호 검사는 Task 4에서 더한다.
- `pub(crate) fn save(path: &Path, file: &LedgerFile) -> Result<(), LedgerError>`: 부모 폴더 `create_dir_all` → `format!("{}.tmp", Ledger::FILE_NAME)`를 `File::create`, `serde_json::to_vec_pretty`를 `write_all`, `sync_all` → `std::fs::rename(tmp, path)` → `#[cfg(unix)]` 부모 폴더를 `File::open`해 `sync_all`(Windows는 폴더 fsync가 std로 안 되며 NTFS 메타데이터 저널에 맡긴다). 입출력 오류는 동작 이름(`"create_dir_all"`, `"write"`, `"sync_all"`, `"rename"`)과 경로로 `rf_core::Error::io`.

`Ledger` 메서드:
- `FILE_NAME = "ledger.json"`, `FORMAT = 1`, `file_path(paths) = paths.data_dir().join(FILE_NAME)`.
- `open(paths, backend)`: `load` → 있으면 항목과 `next_id = max(file.next_id, 최대 id + 1)`, 없으면 빈 원장·`next_id = 1`. 아무것도 만들지 않는다. 이 태스크에서는 `ReconcileReport::default()`를 돌려준다(조정은 Task 4).
- `inspect(paths)`: `load`만 한다(백엔드 없음). 없으면 빈 목록.
- `entries()`: id 오름차순 슬라이스.
- `apply(group, change)` 순서(테스트가 이 순서에 의존한다):
  1. `group`이 비었거나 `[a-z0-9.-]` 밖의 글자가 있으면 `InvalidGroup(group)`.
  2. `change.validate()?`.
  3. 살아 있는 항목(`Pending` 포함) 중 `same_slot`이면서 그룹이 다른 것이 있으면 `SlotOwned { target: change.target.to_string(), owner }`.
  4. `backend.is_applied(&change)?`가 `true`면 `Ok(ApplyOutcome::Unchanged)`(기록하지 않는다. 바꾸기 전 값이 원하는 값과 같으면 되돌릴 것도 없다).
  5. `before = backend.capture(&change)?`.
  6. `Pending` 항목을 `next_id`로 넣고 `next_id += 1`, `save`(fsync까지). 저장이 실패하면 항목을 빼고 그 오류를 돌려준다(OS는 아직 그대로).
  7. `backend.apply(&change)`가 실패하면 `backend.restore(&change, &before)`를 부른다. 성공하면 항목을 빼고 `save`(이 저장의 실패는 무시: 다음 `open`의 조정이 처리), 실패하면 항목을 `Pending`으로 둔다. 어느 쪽이든 원래 apply 오류를 돌려준다.
  8. 항목을 `Applied`로 바꾸고 `save`. 이 저장의 실패는 무시한다(디스크에는 `Pending`으로 남고 다음 `open`의 조정이 `Applied`로 확인한다). `Ok(ApplyOutcome::Recorded(id))`.
  `change`와 `before`는 6단계에서 항목 안으로 옮기고, 7단계는 `self.entries.last()`의 것을 참조로 넘긴다(복제하면 clippy pedantic `needless_pass_by_value`가 걸린다).

lib.rs에 `mod store; pub mod ledger;`와 Interfaces의 재수출을 추가한다.

- [ ] **Step 7: 통과를 확인한다**

Run: `cargo test -p rf-ledger --test ledger_apply`
Expected: Linux `test result: ok. 12 passed`, Windows·macOS `test result: ok. 11 passed`(패닉 주입 테스트가 stderr에 `injected crash`를 찍는 것은 정상)

- [ ] **Step 8: 표준 검사와 교차 검사를 돌린다**

- [ ] **Step 9: 커밋한다**

```bash
git add Cargo.lock crates/rf-ledger
git commit -m "feat(rf-ledger): add write-ahead ledger with fake backend"
```

---

### Task 3: 역순 복원(revert_all, revert_group)

**Files:**
- Modify: `crates/rf-ledger/src/ledger.rs`, `crates/rf-ledger/src/lib.rs`
- Test: `crates/rf-ledger/tests/ledger_revert.rs`

**Interfaces:**
- Consumes: Task 2의 `Ledger`, `SystemBackend::{is_applied, restore}`, `store::save`, `EntryFailure`
- Produces:
  - `#[derive(Clone, Debug, Default, PartialEq, Eq)] #[non_exhaustive] pub struct RevertReport { pub reverted: Vec<EntryId>, pub skipped: Vec<EntryId>, pub failed: Vec<EntryFailure> }`, `pub fn is_clean(&self) -> bool`(`failed`가 비면 `true`). `reverted`·`skipped`·`failed`는 처리 순서(id 내림차순). `skipped`는 그 사이 다른 누군가가 값을 바꿔 원장에서 손만 뗀 항목.
  - `pub fn revert_all(&mut self) -> RevertReport`, `pub fn revert_group(&mut self, group: &str) -> RevertReport`. WINX·MACX 계획은 기능을 끌 때 `revert_group`을 쓴다.
  - 재수출 `rf_ledger::RevertReport`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/ledger_revert.rs`:

```rust
mod common;

use common::*;
use rf_ledger::{EntryId, FakeOp, Ledger};

#[test]
fn revert_all_restores_in_reverse_order() {
    let fx = installed();
    let (x, y) = (slot(1), slot(2));
    fx.fake.set(&x, s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&x, "a")).unwrap();
    l.apply("g", set(&y, "b")).unwrap();
    l.apply("g", set(&x, "c")).unwrap();
    let r = l.revert_all();
    assert_eq!(r.reverted, [EntryId(3), EntryId(2), EntryId(1)]);
    assert!(r.skipped.is_empty() && r.is_clean());
    assert_eq!((fx.fake.get(&x), fx.fake.get(&y)), (s("orig"), None));
    assert!(l.entries().is_empty() && Ledger::inspect(&fx.paths).unwrap().is_empty());
}

#[test]
fn revert_all_twice_is_harmless() {
    let fx = installed();
    let mut l = fx.open();
    l.apply("g", set(&slot(1), "a")).unwrap();
    l.revert_all();
    let calls = fx.fake.calls().len();
    let r = l.revert_all();
    assert!(r.reverted.is_empty() && r.skipped.is_empty() && r.failed.is_empty());
    assert_eq!(fx.fake.calls().len(), calls);
}

#[test]
fn revert_continues_past_a_failure_and_reports_it() {
    let fx = installed();
    let mut l = fx.open();
    for n in 1..=3 {
        l.apply("g", set(&slot(n), "x")).unwrap();
    }
    fx.fake.fail_next(FakeOp::Restore, &slot(2));
    let r = l.revert_all();
    assert_eq!(r.reverted, [EntryId(3), EntryId(1)]);
    assert_eq!((r.failed.len(), r.failed[0].id, r.failed[0].change.as_str()), (1, EntryId(2), r"HKCU\Software\T2 @ v"));
    assert!(r.failed[0].error.contains("injected"), "{}", r.failed[0].error);
    assert!(!r.is_clean());
    assert_eq!(l.entries().len(), 1);
    assert_eq!(l.revert_all().reverted, [EntryId(2)]);
}

#[test]
fn revert_leaves_values_someone_else_changed_since() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("explorer"));
    let mut l = fx.open();
    l.apply("g", set(&t, "ours")).unwrap();
    fx.fake.set(&t, s("other-app"));
    let r = l.revert_all();
    assert_eq!((r.reverted.len(), r.skipped.clone()), (0, vec![EntryId(1)]));
    assert_eq!(fx.fake.get(&t), s("other-app"));
    assert!(l.entries().is_empty());
}

#[test]
fn revert_group_touches_only_that_group() {
    let fx = installed();
    let mut l = fx.open();
    l.apply("winx.a", set(&slot(1), "a")).unwrap();
    l.apply("macx.b", set(&slot(2), "b")).unwrap();
    assert_eq!(l.revert_group("winx.a").reverted, [EntryId(1)]);
    assert_eq!((fx.fake.get(&slot(1)), fx.fake.get(&slot(2))), (None, s("b")));
    assert_eq!(l.entries()[0].group, "macx.b");
}

#[test]
fn pending_entries_are_restored_without_the_ownership_check() {
    let fx = installed();
    let t = slot(1);
    fx.fake.set(&t, s("orig"));
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &t);
    fx.fake.fail_next(FakeOp::Restore, &t);
    assert!(l.apply("g", set(&t, "ours")).is_err());
    fx.fake.set(&t, s("half-written"));
    assert_eq!(l.revert_all().reverted, [EntryId(1)]);
    assert_eq!(fx.fake.get(&t), s("orig"));
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test -p rf-ledger --test ledger_revert`
Expected: FAIL, `error[E0599]: no method named `revert_all` found for struct `Ledger``

- [ ] **Step 3: `RevertReport`와 두 메서드를 구현한다**

둘 다 비공개 `fn revert_where(&mut self, pick: impl Fn(&LedgerEntry) -> bool) -> RevertReport`를 부른다. `pick`에 맞는 항목을 id 내림차순으로 하나씩:
1. `Applied`면 먼저 `is_applied`. `Err`면 `failed`(항목 유지). `Ok(false)`면 지금 값이 이 앱이 쓴 값이 아니므로 복원하지 않고 `skipped`에 넣고 항목을 뺀다(다른 앱·사용자의 값을 덮어쓰지 않는다).
2. `Pending`이거나 `is_applied`가 `true`면 `restore(&change, &before)`. `Err`면 `failed`(항목 유지), `Ok`면 `reverted`에 넣고 항목을 뺀다.
3. 항목을 뺄 때마다 `save`한다(도중에 죽어도 디스크가 지금 상태를 가리키게). 저장이 실패하면 그 항목을 메모리에 되돌려 두고 `failed`에 `error: format!("restored, but the ledger file could not be updated: {e}")`로 넣는다.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test -p rf-ledger --test ledger_revert`
Expected: `test result: ok. 6 passed`

- [ ] **Step 5: 표준 검사를 돌린다**

- [ ] **Step 6: 커밋한다**

```bash
git add crates/rf-ledger
git commit -m "feat(rf-ledger): revert entries in reverse order with a report"
```

---

### Task 4: 시작 시 조정과 원장 파일 보호

**Files:**
- Modify: `crates/rf-ledger/src/ledger.rs`, `crates/rf-ledger/src/store.rs`
- Test: `crates/rf-ledger/tests/ledger_recovery.rs`

**Interfaces:**
- Consumes: Task 2의 `open`, `store::{load, save}`, `ReconcileReport`; Task 3의 복원 규칙
- Produces: `Ledger::open`이 돌려주는 `ReconcileReport`가 실제 조정 결과를 담는다. `Ledger::open`·`Ledger::inspect`는 손상 파일에 `Corrupt`, 더 새 형식에 `UnsupportedFormat`을 돌려주고 파일을 건드리지 않는다(Task 6의 `plan_cleanup`·`run_cleanup`도 이 둘을 거쳐 같은 오류를 낸다). 앱은 `open`의 `Err`를 사용자에게 보이고 원장 기능(탐색기 대체 등) 변경을 막는다(UI 계획).

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/ledger_recovery.rs`:

```rust
mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use common::*;
use rf_ledger::{EntryId, EntryState, FakeOp, Ledger, LedgerError};

fn reopen(fx: &Fx) -> (Ledger, rf_ledger::ReconcileReport) {
    Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap()
}

/// Leaves entry 1 `Pending` on disk while the live slot still holds `orig`.
fn pending_not_applied(fx: &Fx) {
    fx.fake.set(&slot(1), s("orig"));
    let mut l = fx.open();
    fx.fake.fail_next(FakeOp::Apply, &slot(1));
    fx.fake.fail_next(FakeOp::Restore, &slot(1));
    assert!(l.apply("g", set(&slot(1), "ours")).is_err());
}

#[test]
fn pending_entry_whose_change_is_live_is_confirmed() {
    let fx = installed();
    let mut l = fx.open();
    fx.fake.panic_after_next(FakeOp::Apply, &slot(1));
    assert!(catch_unwind(AssertUnwindSafe(|| l.apply("g", set(&slot(1), "ours")))).is_err());
    drop(l);
    let (l, report) = reopen(&fx);
    assert_eq!(report.confirmed, [EntryId(1)]);
    assert!(report.rolled_back.is_empty() && report.failed.is_empty());
    assert_eq!(l.entries()[0].state, EntryState::Applied);
    assert_eq!(Ledger::inspect(&fx.paths).unwrap()[0].state, EntryState::Applied);
}

#[test]
fn pending_entry_whose_change_is_not_live_is_rolled_back() {
    let fx = installed();
    pending_not_applied(&fx);
    let (l, report) = reopen(&fx);
    assert_eq!(report.rolled_back, [EntryId(1)]);
    assert!(l.entries().is_empty() && Ledger::inspect(&fx.paths).unwrap().is_empty());
    assert_eq!(fx.fake.get(&slot(1)), s("orig"));
}

#[test]
fn reconcile_failure_keeps_the_entry_pending() {
    let fx = installed();
    pending_not_applied(&fx);
    fx.fake.fail_next(FakeOp::Restore, &slot(1));
    let (l, report) = reopen(&fx);
    assert_eq!(report.failed[0].id, EntryId(1));
    assert_eq!(l.entries()[0].state, EntryState::Pending);
}

#[test]
fn corrupt_ledger_is_never_overwritten() {
    let fx = installed();
    let file = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let text = br#"{"format":1,"next_id":2,"entries":["#;
    std::fs::write(&file, text).unwrap();
    assert!(matches!(
        Ledger::open(&fx.paths, Box::new(fx.fake.clone())),
        Err(LedgerError::Corrupt { ref path, .. }) if *path == file
    ));
    assert!(matches!(Ledger::inspect(&fx.paths), Err(LedgerError::Corrupt { .. })));
    assert_eq!(std::fs::read(&file).unwrap(), text);
    assert!(fx.fake.calls().is_empty());
}

#[test]
fn newer_format_is_refused_untouched() {
    let fx = installed();
    let file = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let text = br#"{"format":2,"next_id":1,"entries":[]}"#;
    std::fs::write(&file, text).unwrap();
    let err = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap_err();
    assert!(matches!(err, LedgerError::UnsupportedFormat { found: 2, supported: 1, .. }), "{err:?}");
    assert_eq!(Ledger::FORMAT, 1);
    assert_eq!(std::fs::read(&file).unwrap(), text);
}

#[test]
fn leftover_temp_file_is_ignored_and_replaced() {
    let fx = installed();
    fx.open().apply("g", set(&slot(1), "a")).unwrap();
    std::fs::write(fx.paths.data_dir().join("ledger.json.tmp"), b"garbage").unwrap();
    let mut l = fx.open();
    assert_eq!(l.entries().len(), 1);
    l.apply("g", set(&slot(2), "b")).unwrap();
    assert_eq!(files_under(fx.paths.data_dir()), [Ledger::file_path(&fx.paths)]);
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test -p rf-ledger --test ledger_recovery`
Expected: FAIL, `test result: FAILED. 2 passed; 4 failed` (조정 3개와 `newer_format_is_refused_untouched`가 실패한다. `corrupt_ledger_is_never_overwritten`과 `leftover_temp_file_is_ignored_and_replaced`는 Task 2의 저장소 동작을 고정하는 회귀 테스트라 이미 통과한다)

- [ ] **Step 3: 형식 검사와 조정을 구현한다**

- `store::load`: 본문을 파싱하기 전에 비공개 `#[derive(Deserialize)] struct Header { format: u32 }`로 먼저 읽는다(모르는 필드 무시). 헤더 파싱이 실패하면 `Corrupt`, `format > Ledger::FORMAT`이면 `UnsupportedFormat { path, found, supported: Ledger::FORMAT }`, 그 뒤 본문 파싱 실패는 `Corrupt`. 어떤 오류에서도 파일을 쓰거나 지우지 않는다.
- `Ledger::open`: 불러온 뒤 `Pending` 항목을 id 내림차순으로 조정한다. `is_applied`가 `Ok(true)`면 `Applied`로 바꾸고 `confirmed`, `Ok(false)`면 `restore(&change, &before)`(일부만 적용됐을 수 있으니 무조건 바꾸기 전 상태로) → `Ok`면 항목을 빼고 `rolled_back`, `Err`면 항목을 `Pending`으로 두고 `failed`. `is_applied`의 `Err`도 `failed`. 바뀐 것이 있으면 한 번 `save`하고, 그 저장이 실패하면 `open`이 그 오류를 돌려준다(다음 `open`이 같은 조정을 다시 해도 결과가 같다). `Applied` 항목은 조정하지 않는다.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test -p rf-ledger --test ledger_recovery`
Expected: `test result: ok. 6 passed`
Run: `cargo test -p rf-ledger`
Expected: 모든 `test result:` 줄이 `ok.`

- [ ] **Step 5: 표준 검사를 돌린다**

- [ ] **Step 6: 커밋한다**

```bash
git add crates/rf-ledger
git commit -m "feat(rf-ledger): reconcile pending entries and protect the ledger file"
```

---

### Task 5: 포터블 모드 차단(INST-06)

**Files:**
- Modify: `crates/rf-ledger/src/ledger.rs`
- Test: `crates/rf-ledger/tests/portable.rs`

**Interfaces:**
- Consumes: Task 2~4의 `Ledger`, rf-core `InstallMode::Portable`
- Produces: 포터블 모드에서 `open`은 파일을 읽지 않고 빈 원장과 빈 `ReconcileReport`, `inspect`는 빈 목록, `apply`는 무엇보다 먼저 `Err(LedgerError::PortableMode)`, `revert_all`·`revert_group`은 빈 `RevertReport`를 돌려준다. 어느 경우에도 백엔드를 부르지 않는다. 기능 계획(WINX 등)은 포터블에서 그 기능을 UI에서 끄되, 실수로 `apply`를 불러도 OS는 바뀌지 않는다.

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/portable.rs`:

```rust
mod common;

use common::*;
use rf_ledger::{Ledger, LedgerError};

#[test]
fn portable_mode_never_calls_the_backend() {
    let fx = portable();
    let (mut l, report) = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap();
    assert!(report.confirmed.is_empty() && report.rolled_back.is_empty() && report.failed.is_empty());
    assert!(matches!(l.apply("g", set(&slot(1), "x")), Err(LedgerError::PortableMode)));
    assert!(l.revert_all().reverted.is_empty() && l.revert_group("g").reverted.is_empty());
    assert!(fx.fake.calls().is_empty());
    assert!(!fx.paths.data_dir().exists());
}

#[test]
fn portable_mode_ignores_a_copied_installed_ledger() {
    let installed_fx = installed();
    installed_fx.open().apply("g", set(&slot(1), "ours")).unwrap();
    let fx = portable();
    let copied = Ledger::file_path(&fx.paths);
    std::fs::create_dir_all(copied.parent().unwrap()).unwrap();
    std::fs::copy(Ledger::file_path(&installed_fx.paths), &copied).unwrap();
    let text = std::fs::read(&copied).unwrap();
    assert!(Ledger::inspect(&fx.paths).unwrap().is_empty());
    let (mut l, _) = Ledger::open(&fx.paths, Box::new(fx.fake.clone())).unwrap();
    assert!(l.entries().is_empty() && l.revert_all().reverted.is_empty());
    assert!(fx.fake.calls().is_empty());
    assert_eq!(std::fs::read(&copied).unwrap(), text);
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test -p rf-ledger --test portable`
Expected: FAIL, `test result: FAILED. 0 passed; 2 failed` (포터블에서도 백엔드가 불리고 복사된 원장을 읽는다)

- [ ] **Step 3: Interfaces의 네 지점에 포터블 분기를 넣는다**

`open`은 `paths.mode()`가 `Portable`이면 `load`와 조정을 건너뛴다. `inspect`는 같은 조건에서 바로 빈 목록. `apply`는 1단계(그룹 검사)보다 앞에서 `PortableMode`. `revert_where`는 맨 앞에서 빈 보고서.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test -p rf-ledger --test portable`
Expected: `test result: ok. 2 passed`

- [ ] **Step 5: 표준 검사를 돌린다**

- [ ] **Step 6: 커밋한다**

```bash
git add crates/rf-ledger
git commit -m "feat(rf-ledger): never touch the OS in portable mode"
```

---

### Task 6: 클린 삭제 코어(plan_cleanup, run_cleanup)

**Files:**
- Create: `crates/rf-ledger/src/cleanup.rs`
- Modify: `crates/rf-ledger/src/ledger.rs` (`backend_mut`), `crates/rf-ledger/src/lib.rs`
- Test: `crates/rf-ledger/tests/cleanup.rs`

**Interfaces:**
- Consumes: Task 2~5의 `Ledger::{open, inspect, revert_all}`, `SystemBackend::clear_domain`; rf-core `AppPaths::{owned_paths, temp_dir, preferences_file, mode}`, `identity::{TEMP_PREFIX, BUNDLE_ID}`
- Produces:
  - `#[derive(Clone, Debug, PartialEq, Eq)] #[non_exhaustive] pub enum CleanupAction { Revert { id: EntryId, change: String }, ClearDomain(String), Delete(PathBuf) }`
  - `#[derive(Clone, Debug, Default, PartialEq, Eq)] #[non_exhaustive] pub struct CleanupReport { pub reverted: RevertReport, pub done: Vec<CleanupAction>, pub failed: Vec<(CleanupAction, String)>, pub deletes_skipped: bool }`, `pub fn is_complete(&self) -> bool`(`reverted.is_clean() && failed.is_empty() && !deletes_skipped`)
  - `pub fn plan_cleanup(paths: &AppPaths) -> Result<Vec<CleanupAction>, LedgerError>`: dry-run. 원장 파일과 폴더 목록만 읽고 OS·파일을 바꾸지 않는다.
  - `pub fn run_cleanup(paths: &AppPaths, backend: Box<dyn SystemBackend>) -> Result<CleanupReport, LedgerError>`: PRD INST-04의 (2) 원장 역순 복원과 (4) 설정·캐시·로그·썸네일 캐시·메타 DB·색인 스냅샷 삭제(모두 `owned_paths()` 안), 그리고 `TEMP_PREFIX` 임시 항목 삭제. 호출자(제거 프로그램, macOS "완전 삭제…" 메뉴)는 먼저 다른 프로세스를 끝내고(INST-04 (1)) 자기 로그 파일을 닫는다(Windows는 열린 파일을 지울 수 없다).
  - 크레이트 내부: `Ledger::backend_mut(&mut self) -> &mut dyn SystemBackend` (`pub(crate)`)
  - 재수출 `rf_ledger::{CleanupAction, CleanupReport, plan_cleanup, run_cleanup}`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/cleanup.rs`:

```rust
mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use common::*;
use rf_core::identity::{BUNDLE_ID, PORTABLE_MARKER, TEMP_PREFIX};
use rf_ledger::{CleanupAction, EntryId, FakeOp, Ledger, LedgerError, plan_cleanup, run_cleanup};

/// Two applied changes plus files in every owned folder and the OS temp folder.
/// Returns the paths a cleanup must delete, in plan order.
fn used(fx: &Fx) -> Vec<PathBuf> {
    fx.fake.set(&slot(1), s("orig"));
    let mut l = fx.open();
    l.apply("g", set(&slot(1), "a")).unwrap();
    l.apply("g", set(&slot(2), "b")).unwrap();
    for (dir, name) in [
        (fx.paths.config_dir(), "settings.toml"),
        (fx.paths.cache_dir(), "thumb.bin"),
        (fx.paths.log_dir(), "app.log"),
    ] {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    let temp = fx.paths.temp_dir();
    std::fs::write(temp.join(format!("{TEMP_PREFIX}1.tmp")), b"x").unwrap();
    std::fs::create_dir_all(temp.join(format!("{TEMP_PREFIX}dir"))).unwrap();
    std::fs::write(temp.join("other.tmp"), b"x").unwrap();
    let mut deletes = fx.paths.owned_paths().to_vec();
    deletes.push(temp.join(format!("{TEMP_PREFIX}1.tmp")));
    deletes.push(temp.join(format!("{TEMP_PREFIX}dir")));
    deletes
}

fn as_deletes(paths: &[PathBuf]) -> Vec<CleanupAction> {
    paths.iter().cloned().map(CleanupAction::Delete).collect()
}

fn cleanup(fx: &Fx) -> rf_ledger::CleanupReport {
    run_cleanup(&fx.paths, Box::new(fx.fake.clone())).unwrap()
}

#[test]
fn dry_run_lists_reverts_then_deletes_and_changes_nothing() {
    let fx = installed();
    let deletes = used(&fx);
    let calls = fx.fake.calls().len();
    let mut want = vec![
        CleanupAction::Revert { id: EntryId(2), change: r"HKCU\Software\T2 @ v".into() },
        CleanupAction::Revert { id: EntryId(1), change: r"HKCU\Software\T1 @ v".into() },
    ];
    want.extend(as_deletes(&deletes));
    assert_eq!(plan_cleanup(&fx.paths).unwrap(), want);
    assert_eq!(fx.fake.calls().len(), calls);
    assert!(deletes.iter().all(|p| p.exists()));
    assert_eq!(Ledger::inspect(&fx.paths).unwrap().len(), 2);
}

#[test]
fn cleanup_reverts_then_deletes_owned_paths_and_prefixed_temp_entries() {
    let fx = installed();
    let deletes = used(&fx);
    let r = cleanup(&fx);
    assert_eq!(r.reverted.reverted, [EntryId(2), EntryId(1)]);
    assert_eq!(r.done, as_deletes(&deletes));
    assert!(r.is_complete() && !r.deletes_skipped);
    assert_eq!((fx.fake.get(&slot(1)), fx.fake.get(&slot(2))), (s("orig"), None));
    assert!(deletes.iter().all(|p| p.symlink_metadata().is_err()));
    assert!(fx.paths.temp_dir().join("other.tmp").exists());
}

#[test]
fn cleanup_keeps_everything_when_a_revert_fails() {
    let fx = installed();
    let deletes = used(&fx);
    fx.fake.fail_next(FakeOp::Restore, &slot(1));
    let r = cleanup(&fx);
    assert!(r.deletes_skipped && r.done.is_empty() && !r.is_complete());
    assert_eq!(r.reverted.failed[0].id, EntryId(1));
    assert!(deletes.iter().all(|p| p.exists()));
    assert!(cleanup(&fx).is_complete());
    assert_eq!(fx.fake.get(&slot(1)), s("orig"));
}

#[test]
fn cleanup_finishes_after_a_crash_mid_revert() {
    let fx = installed();
    let deletes = used(&fx);
    fx.fake.panic_after_next(FakeOp::Restore, &slot(2));
    assert!(catch_unwind(AssertUnwindSafe(|| cleanup(&fx))).is_err());
    let r = cleanup(&fx);
    assert!(r.is_complete(), "{r:?}");
    assert_eq!((fx.fake.get(&slot(1)), fx.fake.get(&slot(2))), (s("orig"), None));
    assert!(deletes.iter().all(|p| p.symlink_metadata().is_err()));
}

#[test]
fn cleanup_refuses_to_delete_when_the_ledger_is_unreadable() {
    let fx = installed();
    let deletes = used(&fx);
    std::fs::write(Ledger::file_path(&fx.paths), b"{").unwrap();
    assert!(matches!(run_cleanup(&fx.paths, Box::new(fx.fake.clone())), Err(LedgerError::Corrupt { .. })));
    assert!(matches!(plan_cleanup(&fx.paths), Err(LedgerError::Corrupt { .. })));
    assert!(deletes.iter().all(|p| p.exists()));
}

#[cfg(unix)]
#[test]
fn cleanup_removes_links_but_not_what_they_point_to() {
    let fx = installed();
    let outside = fx.root().join("elsewhere");
    for (sub, file) in [("cache", "keep.bin"), ("config", "keep.toml")] {
        std::fs::create_dir_all(outside.join(sub)).unwrap();
        std::fs::write(outside.join(sub).join(file), b"x").unwrap();
    }
    std::fs::create_dir_all(fx.paths.cache_dir().parent().unwrap()).unwrap();
    std::fs::create_dir_all(fx.paths.config_dir().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(outside.join("cache"), fx.paths.cache_dir()).unwrap();
    std::os::unix::fs::symlink(outside.join("config"), fx.paths.config_dir()).unwrap();
    let r = cleanup(&fx);
    assert!(r.is_complete(), "{r:?}");
    assert!(fx.paths.owned_paths().iter().all(|p| p.symlink_metadata().is_err()));
    assert!(outside.join("cache").join("keep.bin").exists());
    assert!(outside.join("config").join("keep.toml").exists());
}

#[test]
fn portable_cleanup_deletes_only_the_data_folder() {
    let fx = portable();
    std::fs::create_dir_all(fx.paths.cache_dir()).unwrap();
    let exe = fx.root().join("bin").join("app.exe");
    std::fs::write(&exe, b"exe").unwrap();
    let r = cleanup(&fx);
    assert_eq!(r.done, as_deletes(fx.paths.owned_paths()));
    assert!(exe.exists() && fx.root().join("bin").join(PORTABLE_MARKER).exists());
    assert!(fx.fake.calls().is_empty());
}

#[test]
fn cleanup_twice_is_harmless() {
    let fx = installed();
    used(&fx);
    assert!(cleanup(&fx).is_complete());
    let again = cleanup(&fx);
    assert!(again.is_complete() && again.done.is_empty() && again.reverted.reverted.is_empty());
}

#[test]
fn macos_cleanup_clears_the_preferences_domain_before_deleting_the_plist() {
    let fx = macos();
    let plist = fx.paths.preferences_file().unwrap().to_path_buf();
    std::fs::create_dir_all(plist.parent().unwrap()).unwrap();
    std::fs::write(&plist, b"plist").unwrap();
    let plan = plan_cleanup(&fx.paths).unwrap();
    assert_eq!(plan, [CleanupAction::ClearDomain(BUNDLE_ID.into()), CleanupAction::Delete(plist.clone())]);
    let r = cleanup(&fx);
    assert!(r.is_complete() && r.done == plan);
    assert_eq!(fx.fake.calls(), [FakeOp::ClearDomain]);
    assert!(!plist.exists());
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test -p rf-ledger --test cleanup`
Expected: FAIL, `error[E0432]: unresolved imports` (`rf_ledger::CleanupAction`, `rf_ledger::plan_cleanup`, `rf_ledger::run_cleanup`)

- [ ] **Step 3: `cleanup.rs`를 구현하고 `Ledger::backend_mut`와 재수출을 추가한다**

`plan_cleanup(paths)`가 만드는 목록(이 순서):
1. `Ledger::inspect(paths)?`의 항목을 id 내림차순으로 `Revert { id, change: entry.change.target.to_string() }`(`Pending` 포함). 손상·새 형식이면 여기서 `Err`.
2. `paths.preferences_file()`이 `Some`이고 포터블이 아니면 `ClearDomain(BUNDLE_ID)`. macOS의 cfprefsd는 기본값을 캐시해 두었다가 plist 파일을 다시 쓸 수 있으므로, 파일을 지우기 전에 도메인을 cfprefsd를 통해 비운다. 파일이 없어도 넣는다(캐시에만 남아 있을 수 있다).
3. `owned_paths()` 순서대로 `symlink_metadata`가 성공하는 경로만 `Delete`.
4. `temp_dir()`가 어떤 owned path 아래도 아니면(설치형·macOS), 그 폴더의 항목 중 이름이 `TEMP_PREFIX`로 시작하는 것을 이름순으로 `Delete`. 폴더가 없으면 건너뛴다. 포터블은 `R/tmp`가 `R`과 함께 지워지므로 건너뛴다.

`run_cleanup(paths, backend)`:
1. `Ledger::open(paths, backend)?`(손상·새 형식이면 `Err`로 끝나 아무것도 지우지 않는다).
2. `reverted = ledger.revert_all()`. `is_clean()`이 아니면 원장이 남아야 다시 시도할 수 있으므로 `deletes_skipped: true`로 바로 돌려준다.
3. `plan_cleanup(paths)?`를 다시 만들어(이제 `Revert`는 없다) 순서대로 실행한다. `ClearDomain(d)` → `ledger.backend_mut().clear_domain(&d)`. `Delete(p)` → `symlink_metadata(p)`가 일반 파일이면 `remove_file`, 그 밖(폴더, 심볼릭 링크, 정션)은 `remove_dir_all`(std 문서대로 링크를 따라가지 않고 링크 자체만 지운다). `NotFound`는 성공으로 친다. 성공은 `done`, 실패는 `failed`에 `(action, error.to_string())`. 실패해도 다음 항목을 계속한다.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test -p rf-ledger --test cleanup`
Expected: Linux·macOS `test result: ok. 9 passed`, Windows `test result: ok. 8 passed`

- [ ] **Step 5: 표준 검사를 돌린다**

- [ ] **Step 6: 커밋한다**

```bash
git add crates/rf-ledger
git commit -m "feat(rf-ledger): add clean-uninstall plan and run"
```

---

### Task 7: "모든 OS 변경은 원장으로" 강제(clippy.toml과 정책 테스트)

**Files:**
- Modify: `clippy.toml` (M1-01이 만든 파일 끝에 덧붙인다. 다시 쓰지 않는다)
- Modify: `crates/rf-ledger/Cargo.toml` (cargo add)
- Test: `crates/rf-ledger/tests/policy.rs`

**Interfaces:**
- Consumes: M1-01의 루트 `clippy.toml`(`allow-print-in-tests`, `allow-dbg-in-tests`, `doc-valid-idents`)
- Produces: 워크스페이스 전체에서 아래 API를 직접 부르면 `clippy::disallowed_methods`가 경고(CI에서는 `-D warnings`로 오류)한다. 억제는 `crates/rf-ledger` 안에서만 허용되고 `tests/policy.rs`가 이를 검사한다. 후속 계획이 OS 설정을 바꿔야 하면 rf-ledger에 변경 종류와 백엔드를 더하는 태스크를 둔다. 다른 계획이 이미 `disallowed-methods` 키를 만들었으면 같은 배열에 항목을 더한다(키를 두 번 쓰면 TOML 오류).

- [ ] **Step 1: 개발 의존성을 추가한다**

Run: `cargo add -p rf-ledger --dev toml`
Expected: `Adding toml v` 로 시작하는 줄

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/policy.rs`:

```rust
//! PRD INST-02: every OS change goes through rf-ledger. Enforced by clippy.toml plus these scans.

use std::path::{Path, PathBuf};

const REQUIRED: &[&str] = &[
    "windows_registry::Key::create",
    "windows_registry::Key::set_bytes",
    "windows_registry::Key::set_string",
    "windows_registry::Key::set_u32",
    "windows_registry::Key::set_value",
    "windows_registry::Key::remove_value",
    "windows_registry::Key::remove_tree",
    "windows::Win32::System::Registry::RegSetValueExW",
    "windows::Win32::System::Registry::RegCreateKeyExW",
    "windows::Win32::System::Registry::RegDeleteTreeW",
    "objc2_core_foundation::CFPreferencesSetValue",
    "objc2_core_foundation::CFPreferencesSetAppValue",
    "objc2_foundation::NSUserDefaults::setObject_forKey",
];

/// OS settings tools that must not be spawned outside rf-ledger.
const TOOLS: &[&str] = &[
    "defaults", "/usr/bin/defaults", "reg", "reg.exe", "launchctl", "/bin/launchctl", "sc", "sc.exe", "schtasks",
    "schtasks.exe",
];

fn workspace() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd {
        let p = e.unwrap().path();
        if p.file_name().is_some_and(|n| n == "target") {
            continue;
        }
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// `.rs` files outside crates/rf-ledger whose text matches `bad`.
fn offenders(bad: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let ledger = workspace().join("crates").join("rf-ledger");
    let mut files = Vec::new();
    rust_files(&workspace().join("crates"), &mut files);
    rust_files(&workspace().join("apps"), &mut files);
    assert!(files.iter().any(|f| f.starts_with(&ledger)), "scan did not reach rf-ledger: {files:?}");
    files
        .into_iter()
        .filter(|f| !f.starts_with(&ledger))
        .filter(|f| bad(&std::fs::read_to_string(f).unwrap()))
        .collect()
}

#[test]
fn clippy_toml_disallows_os_write_apis() {
    let text = std::fs::read_to_string(workspace().join("clippy.toml")).unwrap();
    let table: toml::Table = toml::from_str(&text).unwrap();
    let entries = table
        .get("disallowed-methods")
        .and_then(toml::Value::as_array)
        .expect("clippy.toml has no disallowed-methods");
    let field = |e: &toml::Value, k: &str| e.get(k).and_then(toml::Value::as_str).unwrap_or_default().to_owned();
    for required in REQUIRED {
        let entry = entries.iter().find(|&e| field(e, "path") == *required).unwrap_or_else(|| panic!("{required} missing"));
        assert!(field(entry, "reason").contains("rf-ledger"), "{required}");
    }
    assert!(table.contains_key("doc-valid-idents"), "M1-01 settings were replaced instead of appended");
}

#[test]
fn only_rf_ledger_may_suppress_disallowed_methods() {
    let bad = offenders(|text| text.contains("disallowed_methods"));
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn no_os_settings_tool_is_spawned_outside_rf_ledger() {
    let spawns = |text: &str| {
        text.split("Command::new(").skip(1).any(|rest| {
            rest.strip_prefix('"').and_then(|r| r.split('"').next()).is_some_and(|tool| TOOLS.contains(&tool))
        })
    };
    let bad = offenders(spawns);
    assert!(bad.is_empty(), "{bad:?}");
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test -p rf-ledger --test policy`
Expected: FAIL, `test result: FAILED. 2 passed; 1 failed`, 패닉 메시지 `clippy.toml has no disallowed-methods`

- [ ] **Step 4: 루트 `clippy.toml` 끝에 `disallowed-methods`를 덧붙인다**

기존 세 줄은 그대로 두고 아래 주석과 배열을 더한다. 배열의 각 항목은 다음 형식이고, `path`만 아래 목록 순서대로 바뀐다(모두 38개):

```toml
# PRD INST-02: every OS change goes through rf-ledger. Only crates/rf-ledger may suppress
# these (crates/rf-ledger/tests/policy.rs checks it). `allow-invalid` keeps crates that do
# not link an OS crate, or link it without the feature, free of "unresolved path" warnings.
disallowed-methods = [
    { path = "windows_registry::Key::create", reason = "OS changes must go through rf-ledger (PRD INST-02)", allow-invalid = true },
    # … one line per path below, same reason and allow-invalid …
]
```

- `windows_registry::Key::` 뒤에 `create`, `rename`, `remove_tree`, `remove_value`, `set_u32`, `set_u64`, `set_string`, `set_hstring`, `set_expand_string`, `set_expand_hstring`, `set_multi_string`, `set_value`, `set_bytes`, `raw_set_bytes`, 그리고 `windows_registry::OpenOptions::create` (15개)
- `windows::Win32::System::Registry::` 뒤에 `RegCreateKeyExW`, `RegCreateKeyTransactedW`, `RegSetValueExW`, `RegSetKeyValueW`, `RegDeleteValueW`, `RegDeleteKeyValueW`, `RegDeleteKeyW`, `RegDeleteKeyExW`, `RegDeleteTreeW`, `RegRenameKey`, `RegCopyTreeW` (11개)
- `objc2_core_foundation::` 뒤에 `CFPreferencesSetValue`, `CFPreferencesSetAppValue`, `CFPreferencesSetMultiple` (3개)
- `objc2_foundation::NSUserDefaults::` 뒤에 `setObject_forKey`, `removeObjectForKey`, `setInteger_forKey`, `setFloat_forKey`, `setDouble_forKey`, `setBool_forKey`, `setURL_forKey`, `setPersistentDomain_forName`, `removePersistentDomainForName` (9개)

(windows-registry의 `set_string` 등은 내부에서 `set_bytes`를 부르지만 lint는 우리 코드의 직접 호출만 보므로 메서드마다 항목이 필요하다. 경로가 풀리지 않는 크레이트에서는 clippy가 조용히 넘어가고, 풀리는 크레이트에서 오타는 `allow-invalid` 때문에 경고 없이 무시되므로 Task 8·9의 위반 확인 단계가 실제 동작을 검사한다.)

- [ ] **Step 5: 통과와 경고 없음을 확인한다**

Run: `cargo test -p rf-ledger --test policy`
Expected: `test result: ok. 3 passed`
Run: 표준 검사와 교차 검사(Global Constraints)
Expected: 세 번 모두 `Finished`로 끝나고 `does not refer to` 경고가 없음

- [ ] **Step 6: 커밋한다**

```bash
git add clippy.toml Cargo.lock crates/rf-ledger
git commit -m "build: forbid direct registry and preferences writes outside rf-ledger"
```

---

### Task 8: Windows 레지스트리 백엔드(HKCU 전용)와 CI 통합 테스트

**Files:**
- Create: `crates/rf-ledger/src/backend/windows.rs`
- Modify: `crates/rf-ledger/src/backend/mod.rs`, `crates/rf-ledger/Cargo.toml` (cargo add)
- Test: `crates/rf-ledger/tests/windows_registry.rs`

**Interfaces:**
- Consumes: Task 1의 `hkcu_subkey`, `registry_data`, `Change::validate`; Task 2의 `SystemBackend`, `Ledger`; Task 7의 `clippy.toml` 항목
- Produces: `#[cfg(target_os = "windows")] pub mod windows`에 `#[derive(Clone, Copy, Debug, Default)] pub struct RegistryBackend;`(`impl SystemBackend`). Windows에서 `os_backend()`는 `Box::new(RegistryBackend)`. 바꾸기 전 값은 항상 `Value::Raw { ty, data }`(레지스트리 바이트 그대로)로 잡혀 어떤 형식(REG_NONE 등)이든 정확히 복원된다. `Preference`·`Service` 대상은 `Unsupported`.

- [ ] **Step 1: 의존성을 추가한다**

Run: `cargo add -p rf-ledger windows-registry --target 'cfg(target_os = "windows")'`
Expected: `Adding windows-registry v0.6.` 로 시작하는 줄(더 새 버전이 rust-version 1.85 때문에 무시된다는 `warning: ignoring windows-registry@…` 줄이 함께 나올 수 있다)

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/windows_registry.rs`:

```rust
#![cfg(target_os = "windows")]

mod common;

use rf_core::identity::DATA_DIR_NAME;
use rf_ledger::{Before, Change, Ledger, LedgerError, ServiceKind, Target, Value, os_backend};
use windows_registry::CURRENT_USER;

#[test]
fn revert_removes_the_created_test_key() {
    let root = format!(r"Software\{DATA_DIR_NAME}Test");
    assert!(CURRENT_USER.open(&root).is_err(), r"stale HKCU\{root}: delete it and rerun");
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    let deep = format!(r"HKCU\{root}\a\b");
    l.apply("test", Change::registry(&deep, "s", Some(Value::Str("one".into()))).unwrap()).unwrap();
    l.apply("test", Change::registry(&deep, "n", Some(Value::U32(7))).unwrap()).unwrap();
    l.apply("test", Change::registry(&format!(r"HKCU\{root}\a"), "", Some(Value::Str("x".into()))).unwrap()).unwrap();
    {
        let b = CURRENT_USER.open(format!(r"{root}\a\b")).unwrap();
        assert_eq!((b.get_string("s").unwrap(), b.get_u32("n").unwrap()), ("one".to_owned(), 7));
        assert_eq!(CURRENT_USER.open(format!(r"{root}\a")).unwrap().get_string("").unwrap(), "x");
    }
    assert_eq!(l.entries()[0].before.created_keys, [format!(r"HKCU\{root}"), format!(r"HKCU\{root}\a"), deep]);
    let r = l.revert_all();
    assert!(r.is_clean() && r.reverted.len() == 3, "{r:?}");
    assert!(CURRENT_USER.open(&root).is_err(), r"HKCU\{root} still exists");
}

#[test]
fn restores_a_preexisting_value_of_any_type_exactly() {
    let root = format!(r"Software\{DATA_DIR_NAME}Test-raw");
    let key = format!(r"HKCU\{root}");
    let mut backend = os_backend();
    let seed = Change::registry(&key, "v", Some(Value::Raw { ty: 0, data: vec![1, 2, 3] })).unwrap();
    let seed_before = backend.capture(&seed).unwrap();
    assert_eq!(seed_before, Before { value: None, created_keys: vec![key.clone()] });
    backend.apply(&seed).unwrap();
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    l.apply("test", Change::registry(&key, "v", Some(Value::Str("ours".into()))).unwrap()).unwrap();
    assert!(l.revert_all().is_clean());
    {
        let v = CURRENT_USER.open(&root).unwrap().get_value("v").unwrap();
        assert_eq!((u32::from(v.ty()), v.to_vec()), (0, vec![1, 2, 3]));
    }
    backend.restore(&seed, &seed_before).unwrap();
    assert!(CURRENT_USER.open(&root).is_err());
}

#[test]
fn refuses_other_hives_and_other_kinds_before_touching_the_registry() {
    let mut b = os_backend();
    let hklm = Change { target: Target::Registry { key: r"HKLM\Software\X".into(), name: String::new() }, value: None };
    assert!(matches!(b.capture(&hklm), Err(LedgerError::OutsideHkcu(_))));
    assert!(matches!(b.apply(&hklm), Err(LedgerError::OutsideHkcu(_))));
    let ok = Change::registry(&format!(r"HKCU\Software\{DATA_DIR_NAME}Test-refuse"), "v", None).unwrap();
    let forged = Before { value: None, created_keys: vec![r"HKLM\Software\X".into()] };
    assert!(matches!(b.restore(&ok, &forged), Err(LedgerError::OutsideHkcu(_))));
    assert!(matches!(b.capture(&Change::macos_file_viewer()), Err(LedgerError::Unsupported(_))));
    let svc = Change::service(ServiceKind::WindowsService, "rf-indexd", None).unwrap();
    assert!(matches!(b.capture(&svc), Err(LedgerError::Unsupported(_))));
}
```

- [ ] **Step 3: 실패를 확인한다**

Windows 호스트 Run: `cargo test -p rf-ledger --test windows_registry`
Expected: FAIL, `test result: FAILED. 0 passed; 3 failed` (`os_backend()`가 아직 `UnsupportedBackend`)
Linux 컨테이너 Run: `cargo clippy -p rf-ledger --all-targets --target x86_64-pc-windows-msvc -- -D warnings`
Expected: `Finished`(테스트 파일이 컴파일된다. 실패 실행은 Windows에서만 볼 수 있다)

- [ ] **Step 4: `backend/windows.rs`를 구현하고 `backend/mod.rs`에 `#[cfg(target_os = "windows")] pub mod windows;`와 `os_backend()`의 Windows 갈래를 넣는다**

모듈 첫 줄: `#![expect(clippy::disallowed_methods, reason = "rf-ledger's registry backend is the only code allowed to write the registry (PRD INST-02)")]`. 규칙:
- 모든 메서드는 레지스트리를 부르기 전에 `change.validate()?`를 하고, 대상이 `Target::Registry`가 아니면 `Unsupported(format!("{} is not a registry value", change.target))`, `hkcu_subkey(key)?`로 하위 키를 얻는다. `restore`는 `before.created_keys`도 모두 먼저 `hkcu_subkey`로 검사한다.
- 비공개 도우미: `open_key(subkey) -> Result<Option<Key>, LedgerError>`(`CURRENT_USER.open`, 오류 코드 `e.code().0`이 `-2_147_024_894`(0x80070002 파일 없음) 또는 `-2_147_024_893`(0x80070003 경로 없음)이면 `None`), `read(subkey, name) -> Result<Option<(u32, Vec<u8>)>, LedgerError>`(`get_value`, 같은 코드면 `None`, 결과는 `(u32::from(v.ty()), v.to_vec())`), `write(subkey, name, ty, data)`(`CURRENT_USER.create(subkey)?.set_bytes(name, Type::from(ty), data)`), `remove(subkey, name)`(`remove_value`, 없음은 성공). 그 밖의 오류는 `Backend(format!("{op} {key} @ {name}: {e}"))`.
- `capture`: `value`는 `read` 결과를 `Value::Raw`로. `change.value`가 `Some`이면 하위 키의 접두 경로(`a`, `a\b`, …)를 얕은 것부터 열어 보고, 처음 없는 것부터 끝까지를 `format!("HKCU\\{prefix}")`로 `created_keys`에 넣는다.
- `apply`: `Some(v)` → `registry_data(v)?` → `write`, `None` → `remove`.
- `restore`: `before.value`가 `Some(v)`면 `registry_data(v)?` → `write`, `None`이면 `remove`. 그 뒤 `created_keys`를 깊은 것부터: 없으면 넘어가고, 있고 하위 키와 값이 하나도 없으면(`keys()?.next().is_none() && values()?.next().is_none()`) 핸들을 닫은 뒤 `CURRENT_USER.remove_tree(subkey)`, 비어 있지 않으면 거기서 멈춘다(그 키와 상위 키는 다른 누군가의 것이다).
- `is_applied`: `read(subkey, name)? == change.value.as_ref().map(registry_data).transpose()?`.

- [ ] **Step 5: 통과를 확인한다**

Windows 호스트 Run: `cargo test -p rf-ledger --test windows_registry` 그리고 `reg query "HKCU\Software\RacoonFinderTest"`(키 이름은 `DATA_DIR_NAME` + `Test`, PRD Q1로 이름이 바뀌면 함께 바뀐다)
Expected: `test result: ok. 3 passed`, 그리고 `ERROR: The system was unable to find the specified registry key or value.`(영어가 아닌 Windows는 같은 뜻의 현지화된 문장)
Linux 컨테이너: Step 6의 교차 검사로 컴파일을 확인하고, 실행 결과는 Task 10의 CI에서 확인한다.

- [ ] **Step 6: 표준 검사, 교차 검사, 위반 확인을 돌린다**

Run: 표준 검사와 교차 검사(Global Constraints)
Expected: 모두 `Finished`
위반 확인: `src/backend/windows.rs` 첫 줄의 `#![expect(…)]`를 잠시 지우고
Run: `cargo clippy -p rf-ledger --target x86_64-pc-windows-msvc -- -D warnings`
Expected: FAIL, `error: use of a disallowed method `windows_registry::Key::` 로 시작하는 줄이 하나 이상. 그 줄을 되돌리고 같은 명령이 `Finished`인지 확인한다.

- [ ] **Step 7: 커밋한다**

```bash
git add Cargo.lock crates/rf-ledger
git commit -m "feat(rf-ledger): add HKCU-only Windows registry backend"
```

---

### Task 9: macOS 기본값 백엔드(CFPreferences)와 NSFileViewer

**Files:**
- Create: `crates/rf-ledger/src/backend/macos.rs`
- Modify: `crates/rf-ledger/src/backend/mod.rs`, `crates/rf-ledger/Cargo.toml` (cargo add)
- Test: `crates/rf-ledger/tests/macos_prefs.rs`

**Interfaces:**
- Consumes: Task 1의 `Change::{preference, macos_file_viewer, validate}`, `GLOBAL_DOMAIN`, `FILE_VIEWER_KEY`; Task 2의 `SystemBackend`, `Ledger`; Task 7의 `clippy.toml` 항목
- Produces: `#[cfg(target_os = "macos")] pub mod macos`에 `#[derive(Clone, Copy, Debug, Default)] pub struct PreferencesBackend;`(`impl SystemBackend`, `clear_domain` 포함). macOS에서 `os_backend()`는 `Box::new(PreferencesBackend)`. 범위는 현재 사용자·모든 호스트(`defaults write`와 같다), `GLOBAL_DOMAIN`은 `kCFPreferencesAnyApplication`(`defaults -g`). 바꾸기 전 값은 항상 `Value::Plist`(바이너리 plist)로 잡혀 사전·배열·날짜도 정확히 복원된다. `Registry`·`Service` 대상은 `Unsupported`. MACX 계획은 `ledger.apply("macx.file-viewer", Change::macos_file_viewer())`로 켜고 `revert_group("macx.file-viewer")`로 끈다.

- [ ] **Step 1: 의존성을 추가한다**

Run: `cargo add -p rf-ledger objc2-core-foundation --target 'cfg(target_os = "macos")' --no-default-features -F std,CFArray,CFData,CFError,CFNumber,CFPreferences,CFPropertyList,CFString`
Expected: `Adding objc2-core-foundation v0.3.` 로 시작하는 줄

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`crates/rf-ledger/tests/macos_prefs.rs`:

```rust
#![cfg(target_os = "macos")]

mod common;

use std::process::Command;

use rf_core::identity::BUNDLE_ID;
use rf_ledger::change::{FILE_VIEWER_KEY, GLOBAL_DOMAIN};
use rf_ledger::{Change, Ledger, Value, os_backend};

/// `defaults read <domain> <key>` (`-g` for the global domain); `None` when it does not exist.
fn defaults_read(domain: &str, key: &str) -> Option<String> {
    let mut cmd = Command::new("/usr/bin/defaults");
    cmd.arg("read");
    if domain == GLOBAL_DOMAIN { cmd.arg("-g") } else { cmd.arg(domain) };
    let out = cmd.arg(key).output().unwrap();
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn test_domain(suffix: &str) -> String {
    format!("{BUNDLE_ID}.ledgertest{suffix}")
}

#[test]
fn own_domain_changes_are_visible_to_defaults_and_revert() {
    let d = test_domain("");
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    l.apply("test", Change::preference(&d, "Probe", Some(Value::Str("probe".into()))).unwrap()).unwrap();
    assert_eq!(defaults_read(&d, "Probe").as_deref(), Some("probe"));
    l.apply("test", Change::preference(&d, "Probe", Some(Value::I64(5))).unwrap()).unwrap();
    assert_eq!(defaults_read(&d, "Probe").as_deref(), Some("5"));
    assert!(l.revert_all().is_clean());
    assert_eq!(defaults_read(&d, "Probe"), None);
}

#[test]
fn restores_a_dictionary_value_exactly() {
    let d = test_domain("");
    let xml = br#"<?xml version="1.0" encoding="UTF-8"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>a</key><integer>1</integer></dict></plist>"#;
    let mut backend = os_backend();
    let seed = Change::preference(&d, "Dict", Some(Value::Plist(xml.to_vec()))).unwrap();
    let seed_before = backend.capture(&seed).unwrap();
    backend.apply(&seed).unwrap();
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    l.apply("test", Change::preference(&d, "Dict", Some(Value::Bool(true))).unwrap()).unwrap();
    assert!(l.revert_all().is_clean());
    assert!(defaults_read(&d, "Dict").unwrap().contains("a = 1;"));
    backend.restore(&seed, &seed_before).unwrap();
    assert_eq!(defaults_read(&d, "Dict"), None);
}

#[test]
fn clear_domain_removes_every_key() {
    let d = test_domain(".clear");
    let mut backend = os_backend();
    for k in ["A", "B"] {
        backend.apply(&Change::preference(&d, k, Some(Value::Bool(true))).unwrap()).unwrap();
    }
    assert_eq!(defaults_read(&d, "A").as_deref(), Some("1"));
    backend.clear_domain(&d).unwrap();
    assert_eq!((defaults_read(&d, "A"), defaults_read(&d, "B")), (None, None));
}

#[test]
fn global_file_viewer_roundtrip_on_ci_only() {
    if std::env::var_os("GITHUB_ACTIONS").is_none() {
        eprintln!("skipped: changes the global NSFileViewer default; runs only on CI");
        return;
    }
    let original = defaults_read(GLOBAL_DOMAIN, FILE_VIEWER_KEY);
    let fx = common::installed();
    let (mut l, _) = Ledger::open(&fx.paths, os_backend()).unwrap();
    l.apply("macx.file-viewer", Change::macos_file_viewer()).unwrap();
    assert_eq!(defaults_read(GLOBAL_DOMAIN, FILE_VIEWER_KEY).as_deref(), Some(BUNDLE_ID));
    assert!(l.revert_group("macx.file-viewer").is_clean());
    assert_eq!(defaults_read(GLOBAL_DOMAIN, FILE_VIEWER_KEY), original);
}
```

- [ ] **Step 3: 실패를 확인한다**

macOS 호스트 Run: `cargo test -p rf-ledger --test macos_prefs`
Expected: FAIL, `test result: FAILED. 1 passed; 3 failed` (CI 전용 테스트는 로컬에서 건너뛰어 통과로 보인다. 나머지는 `UnsupportedBackend`라 실패)
Linux 컨테이너 Run: `cargo clippy -p rf-ledger --all-targets --target aarch64-apple-darwin -- -D warnings`
Expected: `Finished`

- [ ] **Step 4: `backend/macos.rs`를 구현하고 `backend/mod.rs`에 `#[cfg(target_os = "macos")] pub mod macos;`와 `os_backend()`의 macOS 갈래를 넣는다**

모듈 첫 두 줄: `#![allow(unsafe_code, reason = "CoreFoundation preferences FFI: CFPreferencesSetValue and CFPropertyList conversion are unsafe")]`, `#![expect(clippy::disallowed_methods, reason = "rf-ledger's preferences backend is the only code allowed to write preferences (PRD INST-02)")]`. 규칙:
- 모든 메서드는 먼저 `change.validate()?`, 대상이 `Target::Preference`가 아니면 `Unsupported(format!("{} is not a macOS preference", change.target))`.
- 애플리케이션 ID: `GLOBAL_DOMAIN`이면 `kCFPreferencesAnyApplication`(`CFRetained`로 들려면 `objc2_core_foundation::Type` 트레잇의 `retain()`), 아니면 `CFString::from_str(domain)`. 사용자 `kCFPreferencesCurrentUser`, 호스트 `kCFPreferencesAnyHost`(extern static 읽기는 `unsafe` 블록과 `// SAFETY:`). 구체 CF 타입은 `CFRetained::<CFType>::from(&*x)`로 `CFType`이 된다.
- `capture`: `CFPreferencesCopyValue` → `None`이면 `Before::default()`, `Some(v)`면 `CFPropertyListCreateData(None, Some(&v), CFPropertyListFormat::BinaryFormat_v1_0, 0, null_mut())`의 바이트로 `Value::Plist`(`None`이면 `Backend(format!("cannot serialize {}", change.target))`).
- 비공개 `to_cf(&Value) -> Result<CFRetained<CFType>, LedgerError>`: `Str` → `CFString::from_str`, `Bool` → `CFBoolean::new`, `I64` → `CFNumber::new_i64`, `Bytes` → `CFData::from_bytes`, `Plist` → `CFPropertyListCreateWithData(None, Some(&CFData::from_bytes(b)), 0, null_mut(), null_mut())`(`None`이면 `InvalidValue("not a property list")`), 그 밖은 `InvalidValue`.
- 비공개 `write(app, key, Option<&CFType>)`: `CFPreferencesSetValue` 뒤 `CFPreferencesSynchronize(app, user, host)`가 `false`면 `Backend(format!("CFPreferencesSynchronize failed for {domain}"))`. `apply`는 `change.value`, `restore`는 `before.value`를 `to_cf`해 쓴다(`None`은 키 삭제).
- `is_applied`: `CFPreferencesCopyValue`와 `to_cf(desired)`를 `CFType`의 `PartialEq`(CFEqual)로 비교, 둘 다 없으면 `true`.
- `clear_domain(domain)`: `Change::preference(domain, "x", None)?`로 도메인 규칙을 검사하고, `CFPreferencesCopyKeyList(app, user, host)`의 키(`CFString`으로 `cast_unchecked`)마다 값 `None`으로 `CFPreferencesSetValue`, 끝에 `CFPreferencesSynchronize`.

- [ ] **Step 5: 통과를 확인한다**

macOS 호스트 Run: `cargo test -p rf-ledger --test macos_prefs` 그리고 `defaults read -g NSFileViewer`
Expected: `test result: ok. 4 passed`, 그리고 `defaults read`가 테스트 전과 같은 값(없었으면 `does not exist` 오류)
Linux 컨테이너: Step 6의 교차 검사로 컴파일을 확인하고, 실행 결과는 Task 10의 CI(`GITHUB_ACTIONS`가 있어 전역 `NSFileViewer` 테스트까지 돈다)에서 확인한다.

- [ ] **Step 6: 표준 검사, 교차 검사, 위반 확인을 돌린다**

Run: 표준 검사와 교차 검사
Expected: 모두 `Finished`
위반 확인: `src/backend/macos.rs`의 `#![expect(clippy::disallowed_methods, …)]` 줄을 잠시 지우고
Run: `cargo clippy -p rf-ledger --target aarch64-apple-darwin -- -D warnings`
Expected: FAIL, `error: use of a disallowed method `objc2_core_foundation::CFPreferencesSetValue``. 되돌리고 `Finished`를 확인한다.

- [ ] **Step 7: 커밋한다**

```bash
git add Cargo.lock crates/rf-ledger
git commit -m "feat(rf-ledger): add macOS preferences backend and NSFileViewer change"
```

---

### Task 10: 최종 교차 검증과 CI 확인

**Files:**
- 수정 없음(실패가 나오면 고친 파일만 커밋)

**Interfaces:**
- Consumes: Task 1~9 전체
- Produces: 네 OS CI와 cargo-deny를 통과한 `rf-ledger` 0.1.0. WINX·MACX·설치 프로그램·IDX 계획의 출발점.

- [ ] **Step 1: 잠금 파일 기준으로 전체 테스트를 돌린다**

Run: `cargo test --workspace --locked`
Expected (Linux): rf-core 줄은 M1-01 Task 10과 같고, 이미 있는 rf-platform·rf-vfs·rf-jobs 줄은 M1-02 Task 14·M1-03 Task 19의 수치와 같으며(이 계획은 M1-01 뒤 어느 때나 실행할 수 있다), rf-ledger는 `tests/change.rs` 9, `tests/cleanup.rs` 9, `tests/ledger_apply.rs` 12, `tests/ledger_recovery.rs` 6, `tests/ledger_revert.rs` 6, `tests/macos_prefs.rs` 0, `tests/policy.rs` 3, `tests/portable.rs` 2, `tests/windows_registry.rs` 0, `unittests src/lib.rs`·문서 테스트 0. 모든 줄이 `test result: ok.`

- [ ] **Step 2: 표준 검사와 두 타깃 교차 검사를 돌린다**

Run: `cargo fmt --all --check` 그리고 `cargo clippy --workspace --all-targets --locked -- -D warnings` 그리고 Global Constraints의 교차 검사 두 명령
Expected: fmt 출력 없음, clippy 세 번 모두 `Finished`

- [ ] **Step 3: cargo-deny를 돌린다**

Run: `cargo deny check`
Expected: `advisories ok, bans ok, licenses ok, sources ok` (새 의존성은 모두 MIT·Apache-2.0·Zlib 계열로 M1-01의 허용 목록 안이다)

- [ ] **Step 4: 브랜치를 올리고 CI를 지켜본다** (원격에 올리는 일이므로 사용자의 확인을 받은 뒤에만 한다)

Run: `git push -u origin HEAD` 그리고 `gh run watch --exit-status $(gh run list --branch "$(git branch --show-current)" --limit 1 --json databaseId --jq '.[0].databaseId')`
Expected: 종료 코드 0, 다섯 작업 모두 `success`. 로그에서 OS 테스트가 실제로 돈 증거를 확인한다: `test (windows-latest)`와 `test (windows-11-arm)`은 `tests/windows_registry.rs`가 `3 passed`, `tests/ledger_apply.rs`가 `11 passed`, `tests/cleanup.rs`가 `8 passed`. `test (macos-15)`는 `tests/macos_prefs.rs`가 `4 passed`이고 `skipped: changes the global NSFileViewer` 줄이 없어야 한다(CI에서는 전역 테스트가 실제로 돈다).

- [ ] **Step 5: 고친 것이 있으면 커밋한다**

```bash
git add -A crates clippy.toml Cargo.lock
git commit -m "fix(rf-ledger): address cross-platform CI findings"
```
