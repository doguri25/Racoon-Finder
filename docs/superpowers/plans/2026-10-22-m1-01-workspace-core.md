# M1-01 워크스페이스와 rf-core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cargo 워크스페이스 루트(빌드·린트·라이선스·CI 규칙)와, 다른 모든 크레이트가 쓰는 `rf-core`(앱 식별 상수, 오류 타입, 스킴 경로 `VPath`, `Entry` 메타데이터, 데이터 위치 `AppPaths`)를 만든다.

**Architecture:** 루트 `Cargo.toml`은 `crates/*`, `apps/*` 글롭과 `[workspace.package]`·`[workspace.lints]` 상속만 정의하므로, 이후 계획은 `crates/` 아래에 크레이트를 추가하기만 하면 되고 루트 `Cargo.toml`은 고치지 않는다(다른 루트 파일은 Global Constraints의 덧붙이기 규칙만 허용한다). `rf-core`는 UI와 OS API에 의존하지 않는 순수 Rust 라이브러리다. Windows 경로 규칙은 UTF-16 단위를 다루는 순수 함수(`winpath`)로 분리해 Linux CI에서도 테스트하고, OS 문자열 확장(`std::os::unix`/`std::os::windows`)을 쓰는 부분만 `cfg`로 나눈다.

**Tech Stack:** Rust stable(edition 2024, MSRV 1.85), thiserror, bitflags, serde, proptest·tempfile·serde_json(개발 의존성), cargo-deny, GitHub Actions.

**Spec:** docs/PRD.md

## Global Constraints

- 언어와 버전: Rust stable, `edition = "2024"`, `rust-version = "1.85"`. MSRV보다 새 std API는 clippy `incompatible_msrv`가 막는다. M0 결정 D2(PRD 8.1 "언어" 줄)가 1.85보다 높은 최소 버전을 정했으면 `rust-version`은 그 값이고, M1 계획들의 "MSRV 1.85" 문장과 버전에 달린 기대 출력(예: `Adding windows-registry v0.6.`)은 1.85 기준으로 쓴 것이라 그 값에 맞춰 읽는다.
- 라이선스: 프로젝트는 `GPL-3.0-or-later`(PRD Q2 제안). 의존성은 GPL-3.0 호환 라이선스만 쓴다(PRD 8.5). `cargo-deny`로 검사한다(SEC-08).
- 지원 OS: Windows 11(x64, ARM64), macOS 15 Apple Silicon(PRD 4.1). Linux는 제품 플랫폼이 아니고(PRD 1.4) 개발 컨테이너와 CI에서만 쓴다. 그래도 플랫폼 독립 코드는 Linux·Windows·macOS 세 곳 모두에서 빌드와 테스트를 통과해야 한다.
- 크레이트 이름과 위치: PRD 8.2 그대로(`rf-core`, `rf-platform`, `rf-vfs`, `rf-jobs`, `rf-ledger` …), 라이브러리는 `crates/<이름>`, 실행 파일은 `apps/<이름>`.
- 제품 표시 이름(`Racoon Finder`)과 데이터 폴더 이름(`RacoonFinder`), 번들 ID, URL 스킴은 `rf_core::identity` 상수에만 쓴다(PRD Q1에서 "Raccoon"으로 바뀔 수 있음). 다른 `.rs` 파일에 제품 이름을 쓰면 Task 1의 테스트가 실패한다. 문서 주석에서도 "the app"이라고 쓴다.
- 의존성은 `cargo add -p <crate> <dep>`(개발 의존성은 `--dev`)로 추가한다. 버전 숫자를 손으로 쓰지 않는다. resolver "3"이 `rust-version` 1.85와 호환되는 버전을 고른다. `Cargo.lock`은 항상 커밋한다(CI가 `--locked`로 돈다).
- 새 크레이트 추가 규칙(후속 계획용): `crates/<이름>/Cargo.toml`은 Task 1의 `rf-core` 매니페스트와 같은 상속 블록(`version.workspace = true` … `publish.workspace = true`)과 `[lints] workspace = true`를 쓴다. `rf-core` 의존은 `cargo add -p <이름> --path crates/rf-core`. 루트 `Cargo.toml`·`rust-toolchain.toml`·`rustfmt.toml`·`.github/workflows/ci.yml`은 고치지 않는다. `clippy.toml`과 `deny.toml`은 기존 내용을 지우거나 바꾸지 않고 항목을 더하기만 한다(M1-04는 `clippy.toml` 끝에 `disallowed-methods`를 덧붙이고, `deny.toml`은 Task 2의 규칙대로 `allow`에 한 줄을 더한다).
- OS 전용 코드: Win32·AppKit 같은 OS API는 `#[cfg(target_os = "windows")]` / `#[cfg(target_os = "macos")]` 뒤에 둔다. std의 OS 문자열 확장 트레잇(`std::os::unix::ffi`, `std::os::windows::ffi`)은 그 모듈의 게이트와 같은 `#[cfg(unix)]` / `#[cfg(windows)]`를 쓴다.
- `unsafe`: 워크스페이스 린트가 `unsafe_code = "deny"`다. FFI가 필요한 크레이트는 해당 모듈 첫 줄에 `#![allow(unsafe_code, reason = "…")]`를 쓰고 모든 `unsafe` 블록에 `// SAFETY:` 주석을 단다. `rf-core`에는 `unsafe`가 없다.
- 오류 메시지의 `Display` 문자열은 로그용 영어다. 사용자에게 보이는 문구는 UI가 `ErrorKind`를 Project Fluent 문자열로 바꿔 보여 준다(I18N-02).
- 커밋 메시지: 영어 conventional commits(예: `feat(rf-core): add VPath`). 코드 블록은 간결하게 썼으니 커밋 전에 `cargo fmt --all`을 돌린다.
- 표준 검사(모든 태스크의 마지막 검증):
  - `cargo fmt --all --check` → 출력 없음, 종료 코드 0
  - `cargo clippy --workspace --all-targets -- -D warnings` → `warning:`·`error:` 줄 없이 `Finished` 줄로 끝남
- 교차 검사(`cfg` 코드를 바꾼 태스크): 호스트가 아닌 두 타깃에 대해 `cargo clippy --workspace --all-targets --target <triple> -- -D warnings` → `Finished`로 끝남. 타깃은 `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-apple-darwin`. 처음 한 번 `rustup target add x86_64-pc-windows-msvc aarch64-apple-darwin`(Windows·macOS 호스트에서는 빠진 타깃을 추가). 링크하지 않으므로 Linux 컨테이너에서도 된다. `cfg(windows)` 테스트의 실행은 CI의 Windows 러너가 맡는다.

## Review Focus

- 이름에 `%41`처럼 이스케이프로 보이는 글자가 든 실제 경로를 주소창이나 설정에 붙여 넣는 경우 → 네이티브 경로 입력은 절대 unescape하지 않고 이름 그대로 연다. (Task 4 `native_input_is_never_unescaped`)
- macOS에서 온 NFD 한글 이름과 같은 글자의 NFC 이름 → `VPath`는 유니코드를 정규화하지 않고 두 경로를 구분해 그대로 보존한다. NFC 변환은 I18N-04의 명시적 작업에서만 한다. (Task 4 `unicode_is_kept_exactly`)
- `/a/b` 폴더를 `/a/bc`로 옮기는 경우 → 문자열 접두사가 같아도 형제 폴더를 자기 하위 폴더로 오판하지 않는다(DND-09, PICK-06의 근거가 되는 `starts_with`). (Task 5 `starts_with_is_component_wise`)
- Windows에서 260자를 넘는 경로, `CON` 같은 예약 이름, 끝에 점이 붙은 이름(`foo.`) → 확장 길이 형식(`\\?\`)으로 글자 그대로 넘겨 다른 파일이나 장치를 가리키지 않는다(COMP-02, XFER-12). (Task 5 `extended_length_form`, `reserved_and_trailing_dot_names_stay_literal`)
- 포터블 모드 실행 → `%APPDATA%`, `%LOCALAPPDATA%`, OS 임시 폴더(`%TEMP%`는 AppData 아래다)를 전혀 건드리지 않고 임시 파일까지 `.\data` 아래에만 만든다(INST-06). (Task 9 `portable_marker_puts_everything_under_data`, `ensure_dirs_creates_only_the_layout`)

---

## File Structure

```
Cargo.toml                    워크스페이스: 멤버 글롭, 공통 package 필드, 린트
Cargo.lock                    잠금 파일(커밋)
rust-toolchain.toml           stable + rustfmt, clippy
rustfmt.toml / clippy.toml    포맷·린트 설정
deny.toml                     cargo-deny: 라이선스 허용 목록, 권고, 출처
.gitignore / .gitattributes   무시 목록, 줄바꿈 LF 고정
.github/workflows/ci.yml      4개 OS 매트릭스 + cargo-deny 작업
apps/README.md                apps/* 글롭이 빈 상태로 실패하지 않게 하는 자리표시자
crates/rf-core/
  Cargo.toml
  src/lib.rs                  모듈 선언과 재수출
  src/identity.rs             제품 이름·폴더 이름·번들 ID 등 상수(이름이 나오는 유일한 파일)
  src/winpath.rs              Windows 경로 접두사 규칙과 WTF-8 변환(순수 함수, 모든 OS에서 테스트)
  src/error.rs                PathError, Error, ErrorKind, Result
  src/vpath/mod.rs            VPath 타입과 공개 메서드
  src/vpath/scheme.rs         Scheme 열거형
  src/vpath/escape.rs         무손실 문자열 인코딩(%XX)
  src/vpath/local.rs          OS별 로컬 경로 정규화, 이름 검사, 확장 길이 변환
  src/entry.rs                Entry, EntryKind, Attributes, FileId, VolumeId
  src/os.rs                   Os 열거형(순수 로직이 따를 OS 규칙)
  src/app_paths.rs            AppPaths, BaseDirs, InstallMode
  tests/identity.rs  tests/vpath_local.rs  tests/vpath_nav.rs  tests/vpath_scheme.rs
  tests/error.rs  tests/entry.rs  tests/app_paths.rs
```

## 후속 계획이 쓰는 공개 API (요약)

모든 타입은 `rf_core` 루트에서도 재수출된다(`rf_core::VPath` 등). 세부 규칙은 각 태스크의 Interfaces에 있다.

```rust
// rf_core::identity
pub const DISPLAY_NAME: &str;      // "Racoon Finder"
pub const DATA_DIR_NAME: &str;     // "RacoonFinder"
pub const BUNDLE_ID: &str;         // "io.github.doguri25.RacoonFinder"
pub const URL_SCHEME: &str;        // "racoonfinder"
pub const TEMP_PREFIX: &str;       // "RacoonFinder-"
pub const PORTABLE_MARKER: &str;   // "portable"
pub const PORTABLE_DATA_DIR: &str; // "data"
pub const VERSION: &str;           // env!("CARGO_PKG_VERSION")

// rf_core::winpath
pub const MAX_EXTENDED_PATH: usize; // 32_767
pub fn to_extended_length(path: &[u16]) -> Option<Vec<u16>>;
pub fn strip_extended_length(path: &[u16]) -> Vec<u16>;
pub fn to_wtf8(units: &[u16]) -> Vec<u8>;
pub fn from_wtf8(bytes: &[u8]) -> Option<Vec<u16>>;

// rf_core::vpath
pub enum Scheme { Local, Archive, Remote, Shell, Collection, Search }
impl Scheme { pub const ALL: &[Scheme]; pub const fn name(self) -> &'static str; pub fn from_name(name: &str) -> Option<Scheme> }
pub struct VPath; // Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Display, FromStr, Serialize, Deserialize
impl VPath {
    pub fn local(path: impl AsRef<Path>) -> Result<VPath, PathError>;
    pub fn new(scheme: Scheme, authority: &str, segments: &[&str]) -> Result<VPath, PathError>;
    pub fn parse(input: &str) -> Result<VPath, PathError>;
    pub fn to_uri(&self) -> String;
    pub fn scheme(&self) -> Scheme;
    pub fn authority(&self) -> Option<&str>;
    pub fn segments(&self) -> Option<&[String]>;
    pub fn as_local(&self) -> Option<&Path>;
    pub fn file_name(&self) -> Option<&OsStr>;
    pub fn parent(&self) -> Option<VPath>;
    pub fn join(&self, name: impl AsRef<OsStr>) -> Result<VPath, PathError>;
    pub fn starts_with(&self, base: &VPath) -> bool;
    pub fn to_extended_length(&self) -> Option<PathBuf>;
}

// rf_core::error
pub type Result<T, E = Error> = std::result::Result<T, E>;
pub enum PathError { Empty, Nul, NotAbsolute(String), UnknownScheme(String), InvalidEscape(String), InvalidName(String), Unsupported(String) } // non_exhaustive
pub enum ErrorKind { NotFound, PermissionDenied, AlreadyExists, InvalidPath, Unsupported, Cancelled, Other } // non_exhaustive
pub enum Error { Io { op: &'static str, path: VPath, source: std::io::Error }, Path(PathError), MissingEnv(&'static str), Unsupported(String), Cancelled } // non_exhaustive
impl Error { pub fn io(op: &'static str, path: VPath, source: std::io::Error) -> Error; pub fn kind(&self) -> ErrorKind }

// rf_core::entry
pub struct Attributes: u32; // bitflags: READONLY HIDDEN SYSTEM ARCHIVE COMPRESSED ENCRYPTED SPARSE OFFLINE REPARSE_POINT
pub enum EntryKind { File, Dir, Symlink }
pub struct VolumeId(pub u64);
pub struct FileId(pub u128);
pub struct Entry { pub name: OsString, pub kind: EntryKind, pub size: u64, pub modified: Option<SystemTime>, pub created: Option<SystemTime>, pub accessed: Option<SystemTime>, pub attributes: Attributes, pub volume_id: Option<VolumeId>, pub file_id: Option<FileId> } // non_exhaustive
impl Entry { pub fn new(name: impl Into<OsString>, kind: EntryKind) -> Entry; pub fn extension(&self) -> Option<&OsStr>; pub fn identity(&self) -> Option<(VolumeId, FileId)> }

// rf_core::os
pub enum Os { Windows, MacOs, Other }
impl Os { pub const CURRENT: Os; }

// rf_core::app_paths
pub enum InstallMode { Installed, Portable }
pub struct BaseDirs { pub home: Option<PathBuf>, pub app_data: Option<PathBuf>, pub local_app_data: Option<PathBuf>, pub temp: PathBuf }
impl BaseDirs { pub fn from_env() -> BaseDirs }
pub struct AppPaths;
impl AppPaths {
    pub fn resolve(os: Os, exe: &Path, base: &BaseDirs) -> Result<AppPaths>;
    pub fn detect() -> Result<AppPaths>;
    pub fn mode(&self) -> InstallMode;
    pub fn config_dir(&self) -> &Path;
    pub fn data_dir(&self) -> &Path;
    pub fn cache_dir(&self) -> &Path;
    pub fn log_dir(&self) -> &Path;
    pub fn temp_dir(&self) -> &Path;
    pub fn preferences_file(&self) -> Option<&Path>;
    pub fn owned_paths(&self) -> &[PathBuf];
    pub fn ensure_dirs(&self) -> Result<()>;
}
```

## 이 계획이 일부러 다루지 않는 PRD 항목

- INST-02 변경 원장: M1 rf-ledger 계획. 원장 파일 위치는 `AppPaths::data_dir()`를 쓴다.
- INST-01, INST-04, INST-05, INST-07: 설치 프로그램과 클린 삭제 계획. 지울 대상 목록은 `AppPaths::owned_paths()`와 `identity::TEMP_PREFIX`로 시작한다.
- INST-03의 "종료할 때 임시 파일 삭제": 앱 수명 주기를 가진 계획(메인 앱). 이 계획은 위치(`temp_dir`)와 접두사만 정한다.
- INST-06의 "탐색기 대체·상주·파일 연결·색인 서비스 끄기": 해당 기능 계획이 `AppPaths::mode() == InstallMode::Portable`을 보고 끈다.
- INST-08 설정 버전·마이그레이션: rf-settings 계획.
- NAV-03 주소창 해석(환경 변수, `~`, `sftp://` URL), OPS-07 경로 복사 형식(UNC, 파일 URL): UI·rf-vfs 계획. `VPath::parse`는 `sftp` 같은 모르는 스킴을 `PathError::UnknownScheme`로 돌려줄 뿐이다.
- NAV-16(shell), ARC-01(archive), REM(remote), FIND-07(collection), IDX(search) 각 스킴의 authority·segment 의미: 각 공급자 계획. 이 계획은 공통 형식 `scheme://authority/segment/…`만 정한다.
- XFER-12 대상 파일 시스템 제약 검사(FAT32 4GB, exFAT 금지 문자, 대소문자 충돌), COMP-02의 폴더별 대소문자 구분: rf-platform·rf-jobs 계획. rf-core는 긴 경로와 유니코드를 손실 없이 담는 것까지만 한다.
- META-03 저장 방식: rf-meta(M3). rf-core는 `VolumeId`/`FileId` 값 타입만 준다.
- I18N-04 NFC 변환: rf-rename 계획.
- PRD 8.2 rf-core의 "이벤트": 목록 변경 이벤트는 rf-vfs(NAV-13), 작업 진행 이벤트는 rf-jobs(XFER-01) 계획이 정의한다.
- PRD 7.1 성능 벤치마크 CI, SEC-04 퍼징: 해당 크레이트 계획.

---

### Task 1: 워크스페이스 루트, rf-core 골격, 앱 식별 상수

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `.gitignore`, `.gitattributes`, `apps/README.md`
- Create: `crates/rf-core/Cargo.toml`, `crates/rf-core/src/lib.rs`, `crates/rf-core/src/identity.rs`
- Test: `crates/rf-core/tests/identity.rs`
- 생성됨: `Cargo.lock`

**Interfaces:**
- Consumes: 없음
- Produces: 워크스페이스 규칙(Global Constraints의 새 크레이트 추가 규칙), `rf_core::identity::{DISPLAY_NAME, DATA_DIR_NAME, BUNDLE_ID, URL_SCHEME, TEMP_PREFIX, PORTABLE_MARKER, PORTABLE_DATA_DIR, VERSION}: &str`

- [ ] **Step 1: 루트 파일을 만든다**

`Cargo.toml`:

```toml
[workspace]
resolver = "3"
# apps/README.md keeps the apps/* glob non-empty until the first app crate exists.
members = ["crates/*", "apps/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "GPL-3.0-or-later"
repository = "https://github.com/doguri25/Racoon-Finder"
publish = false

[workspace.lints.rust]
unsafe_code = "deny"
missing_debug_implementations = "warn"
rust_2018_idioms = { level = "warn", priority = -1 }
unused_qualifications = "warn"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
must_use_candidate = "allow"
missing_errors_doc = "allow"
missing_panics_doc = "allow"
similar_names = "allow"
allow_attributes_without_reason = "warn"
dbg_macro = "warn"
print_stdout = "warn"
print_stderr = "warn"
todo = "warn"
unimplemented = "warn"
undocumented_unsafe_blocks = "warn"
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

`rustfmt.toml`:

```toml
edition = "2024"
max_width = 100
newline_style = "Unix"
use_field_init_shorthand = true
use_try_shorthand = true
```

`clippy.toml`:

```toml
allow-print-in-tests = true
allow-dbg-in-tests = true
doc-valid-idents = ["macOS", "NTFS", "APFS", "ReFS", "exFAT", "WTF-8", "WTF-16", "UTF-16", "FileId", "VolumeId", ".."]
```

`.gitignore`:

```gitignore
/target/
**/*.rs.bk
*.pdb
.DS_Store
Thumbs.db
desktop.ini
.idea/
.vscode/
*.swp
```

`.gitattributes` (Windows 체크아웃에서도 LF를 유지해 `cargo fmt --check`가 깨지지 않게 한다):

```gitattributes
* text=auto eol=lf
*.bat text eol=crlf
*.cmd text eol=crlf
```

`apps/README.md`: 한 단락. 내용은 "실행 파일 크레이트(PRD 8.2의 메인 앱, `rf-open`, `rf-indexd`, `rf-helper`)가 이 폴더에 들어간다. 이 파일은 `apps/*` 글롭이 빈 폴더에서 실패하지 않게 둔 것이며 첫 앱 크레이트가 생긴 뒤에도 지우지 않는다." (Cargo는 아무것도 매칭되지 않는 글롭을 문자 그대로의 경로로 보고 `failed to load manifest`로 멈춘다.)

- [ ] **Step 2: rf-core 매니페스트와 빈 lib.rs를 만든다**

`crates/rf-core/Cargo.toml`:

```toml
[package]
name = "rf-core"
description = "Core types shared by every crate: VPath, Entry, errors, app identity and data paths."
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
publish.workspace = true

[lints]
workspace = true

[dependencies]
```

`crates/rf-core/src/lib.rs`는 크레이트 문서 주석 한 줄만 둔다: `//! Core types shared by every crate: app identity, errors, VPath, Entry, app data paths.` (제품 이름을 쓰지 않는다.)

- [ ] **Step 3: 실패하는 테스트를 쓴다**

`crates/rf-core/tests/identity.rs`:

```rust
use std::path::{Path, PathBuf};

use rf_core::identity;

#[test]
fn identity_values() {
    assert_eq!(identity::DISPLAY_NAME, "Racoon Finder");
    assert_eq!(identity::DATA_DIR_NAME, "RacoonFinder");
    assert_eq!(identity::BUNDLE_ID, "io.github.doguri25.RacoonFinder");
    assert_eq!(identity::URL_SCHEME, "racoonfinder");
    assert_eq!(identity::TEMP_PREFIX, "RacoonFinder-");
    assert_eq!(identity::PORTABLE_MARKER, "portable");
    assert_eq!(identity::PORTABLE_DATA_DIR, "data");
    assert_eq!(identity::VERSION, env!("CARGO_PKG_VERSION"));
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

/// PRD Q1 may rename the product: only `identity.rs` may spell the name.
#[test]
fn product_name_appears_only_in_identity_module() {
    let core = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace = core.parent().unwrap().parent().unwrap();
    let allowed = [core.join("src").join("identity.rs"), core.join("tests").join("identity.rs")];
    let needle = identity::DISPLAY_NAME.split(' ').next().unwrap().to_lowercase();
    let mut files = Vec::new();
    rust_files(&workspace.join("crates"), &mut files);
    rust_files(&workspace.join("apps"), &mut files);
    assert!(files.contains(&allowed[0]), "scan did not reach identity.rs: {files:?}");
    let offenders: Vec<_> = files
        .iter()
        .filter(|f| !allowed.contains(f))
        .filter(|f| std::fs::read_to_string(f).unwrap().to_lowercase().contains(&needle))
        .collect();
    assert!(offenders.is_empty(), "product name outside identity.rs: {offenders:?}");
}
```

- [ ] **Step 4: 실패를 확인한다**

Run: `cargo test -p rf-core --test identity`
Expected: FAIL, `error[E0432]: unresolved import `rf_core::identity``

- [ ] **Step 5: `crates/rf-core/src/identity.rs`를 만들고 lib.rs에 `pub mod identity;`를 추가한다**

값은 스펙이 정한 그대로 쓴다(PRD INST-03 `RacoonFinder`, MACX-02 `racoonfinder://`, INST-06 `portable`·`data`). 번들 ID는 이 계획의 제안이며 Q1 결정 때 이 파일만 바꾼다. 각 상수에 한 줄 문서 주석을 단다.

```rust
pub const DISPLAY_NAME: &str = "Racoon Finder";
pub const DATA_DIR_NAME: &str = "RacoonFinder";
pub const BUNDLE_ID: &str = "io.github.doguri25.RacoonFinder";
pub const URL_SCHEME: &str = "racoonfinder";
pub const TEMP_PREFIX: &str = "RacoonFinder-";
pub const PORTABLE_MARKER: &str = "portable";
pub const PORTABLE_DATA_DIR: &str = "data";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
```

- [ ] **Step 6: 통과를 확인한다**

Run: `cargo test -p rf-core --test identity`
Expected: `test result: ok. 2 passed`

- [ ] **Step 7: 표준 검사를 돌린다** (Global Constraints의 fmt·clippy 두 명령)

- [ ] **Step 8: 커밋한다**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml clippy.toml .gitignore .gitattributes apps/README.md crates/rf-core
git commit -m "chore: set up cargo workspace and rf-core identity constants"
```

---

### Task 2: CI와 cargo-deny 라이선스 정책

**Files:**
- Create: `deny.toml`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: Task 1의 워크스페이스(`publish = false`라서 `private = { ignore = true }`와 `allow-wildcard-paths = true`가 우리 크레이트에 적용된다)
- Produces: CI 작업 이름 `test (ubuntu-latest)`, `test (windows-latest)`, `test (windows-11-arm)`, `test (macos-15)`, `cargo-deny`. 후속 계획은 이 파일들을 고치지 않는다. GPL-3.0과 호환되지만 목록에 없는 라이선스가 필요하면 그 의존성을 추가하는 태스크가 `deny.toml`의 `allow`에 한 줄을 더하고 커밋 메시지에 근거를 적는다.

- [ ] **Step 1: cargo-deny를 설치한다** (이미 있으면 건너뜀)

Run: `cargo deny --version || cargo install --locked cargo-deny`
Expected: `cargo-deny 0.` 으로 시작하는 버전 줄, 또는 `Installed package `cargo-deny v0.…``(소스 빌드라 몇 분 걸린다)

- [ ] **Step 2: 설정 없이 라이선스 검사가 실패하는지 확인한다**

Run: `cargo deny check licenses`
Expected: FAIL, 마지막 줄 `licenses FAILED` (기본 허용 목록이 비어 있어 rf-core의 `GPL-3.0-or-later`가 거부됨)

- [ ] **Step 3: `deny.toml`을 만든다**

cargo-deny 0.18 이후 형식이다. `version` 키와 `unlicensed`·`copyleft`·`default` 키는 없어졌고, 허용 목록에 없는 라이선스와 라이선스 메타데이터가 없는 크레이트는 기본으로 거부된다. GNU 라이선스는 `-only`와 `-or-later`를 따로 적어야 한다.

```toml
# cargo-deny configuration (PRD 7.3 SEC-08, 8.5).

[graph]
targets = [
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
]
all-features = true

[advisories]
yanked = "deny"
unmaintained = "workspace"
ignore = []

[licenses]
# Only GPL-3.0-compatible licenses (PRD 8.5). Anything not listed, and any crate
# without license metadata, is rejected.
allow = [
    "0BSD",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC0-1.0",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "ISC",
    "LGPL-2.1-or-later",
    "LGPL-3.0-or-later",
    "MIT",
    "MIT-0",
    "MPL-2.0",
    "Unicode-3.0",
    "Unlicense",
    "Zlib",
]
confidence-threshold = 0.93
unused-allowed-license = "allow"
private = { ignore = true }

[bans]
multiple-versions = "warn"
wildcards = "deny"
allow-wildcard-paths = true

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo deny check`
Expected: 마지막 줄 `advisories ok, bans ok, licenses ok, sources ok`

- [ ] **Step 5: `.github/workflows/ci.yml`을 만든다**

`dtolnay/rust-toolchain`은 rustup이 없는 러너(windows-11-arm 등)에서 rustup부터 설치한다. `windows-11-arm` 러너는 공개 저장소에서만 쓸 수 있다.

```yaml
name: CI

on:
  push:
  pull_request:
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  RUST_BACKTRACE: "1"

jobs:
  test:
    name: test (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, windows-latest, windows-11-arm, macos-15]
    steps:
      - uses: actions/checkout@v7
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --workspace --all-targets --locked -- -D warnings
      - run: cargo test --workspace --locked

  deny:
    name: cargo-deny
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: EmbarkStudios/cargo-deny-action@v2
        with:
          command: check advisories bans licenses sources
```

- [ ] **Step 6: YAML 구조를 확인한다**

Run: `python3 -c "import yaml; d = yaml.safe_load(open('.github/workflows/ci.yml')); print(sorted(d['jobs']), d['jobs']['test']['strategy']['matrix']['os'])"`
Expected: `['deny', 'test'] ['ubuntu-latest', 'windows-latest', 'windows-11-arm', 'macos-15']` (PyYAML이 없으면 `python3 -m pip install --user pyyaml` 후 다시 실행. 실제 CI 통과는 Task 10에서 확인한다.)

- [ ] **Step 7: 커밋한다**

```bash
git add deny.toml .github/workflows/ci.yml
git commit -m "ci: add cross-platform CI and cargo-deny license policy"
```

---

### Task 3: Windows 경로 접두사와 WTF-8 순수 함수 (`winpath`)

**Files:**
- Create: `crates/rf-core/src/winpath.rs` (모듈 안 `#[cfg(test)] mod tests` 포함)
- Modify: `crates/rf-core/src/lib.rs` (`pub mod winpath;`), `crates/rf-core/Cargo.toml` (cargo add)

**Interfaces:**
- Consumes: 없음
- Produces (모든 OS에서 컴파일되는 순수 함수):
  - `pub const MAX_EXTENDED_PATH: usize = 32_767;`
  - `pub fn to_extended_length(path: &[u16]) -> Option<Vec<u16>>`
  - `pub fn strip_extended_length(path: &[u16]) -> Vec<u16>`
  - `pub fn to_wtf8(units: &[u16]) -> Vec<u8>`
  - `pub fn from_wtf8(bytes: &[u8]) -> Option<Vec<u16>>`

- [ ] **Step 1: 개발 의존성을 추가한다**

Run: `cargo add -p rf-core --dev proptest`
Expected: `Adding proptest v1.` 로 시작하는 줄

- [ ] **Step 2: 테스트만 든 `winpath.rs`를 만들고 lib.rs에 `pub mod winpath;`를 추가한다**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn w(s: &str) -> Vec<u16> { s.encode_utf16().collect() }
    fn s(u: &[u16]) -> String { String::from_utf16(u).unwrap() }
    fn ext(p: &str) -> Option<String> { to_extended_length(&w(p)).map(|u| s(&u)) }

    #[test]
    fn extended_length_of_drive_and_unc_paths() {
        assert_eq!(ext(r"C:\a\b").as_deref(), Some(r"\\?\C:\a\b"));
        assert_eq!(ext("c:/a/b").as_deref(), Some(r"\\?\c:\a\b"));
        assert_eq!(ext(r"C:\").as_deref(), Some(r"\\?\C:\"));
        assert_eq!(ext(r"\\srv\share\x").as_deref(), Some(r"\\?\UNC\srv\share\x"));
        assert_eq!(ext("//srv/share/x").as_deref(), Some(r"\\?\UNC\srv\share\x"));
    }

    #[test]
    fn extended_length_keeps_verbatim_and_device_paths() {
        assert_eq!(ext(r"\\?\C:\a").as_deref(), Some(r"\\?\C:\a"));
        assert_eq!(ext(r"\\.\COM1").as_deref(), Some(r"\\.\COM1"));
    }

    #[test]
    fn extended_length_rejects_relative_forms() {
        for p in ["", r"a\b", "C:", "C:a", r"\a", "a"] {
            assert_eq!(ext(p), None, "{p}");
        }
    }

    #[test]
    fn extended_length_handles_32k_paths_and_lone_surrogates() {
        let mut long = w(r"C:\");
        long.extend(std::iter::repeat_n(u16::from(b'a'), MAX_EXTENDED_PATH - 7));
        assert_eq!(to_extended_length(&long).unwrap().len(), MAX_EXTENDED_PATH);
        let lone = [u16::from(b'C'), u16::from(b':'), u16::from(b'\\'), 0xD800];
        assert_eq!(to_extended_length(&lone).unwrap(), [w(r"\\?\C:\").as_slice(), &[0xD800]].concat());
    }

    #[test]
    fn strip_extended_length_inverts_drive_and_unc() {
        assert_eq!(s(&strip_extended_length(&w(r"\\?\C:\a"))), r"C:\a");
        assert_eq!(s(&strip_extended_length(&w(r"\\?\UNC\srv\share\a"))), r"\\srv\share\a");
        assert_eq!(s(&strip_extended_length(&w(r"\\?\unc\srv\share\a"))), r"\\srv\share\a");
        assert_eq!(s(&strip_extended_length(&w(r"\\?\Volume{1}\a"))), r"\\?\Volume{1}\a");
        assert_eq!(s(&strip_extended_length(&w(r"C:\a"))), r"C:\a");
    }

    #[test]
    fn wtf8_encodes_lone_surrogates() {
        assert_eq!(to_wtf8(&[0xD800]), vec![0xED, 0xA0, 0x80]);
        assert_eq!(from_wtf8(&[0xED, 0xA0, 0x80]), Some(vec![0xD800]));
        assert_eq!(from_wtf8(&[0xFF]), None);
        assert_eq!(from_wtf8(&[0xC0, 0x80]), None);
        assert_eq!(from_wtf8(&[0xE2, 0x82]), None);
        assert_eq!(from_wtf8(&[0xF4, 0x90, 0x80, 0x80]), None);
        assert_eq!(from_wtf8("한".as_bytes()), Some(vec![0xD55C]));
    }

    proptest! {
        #[test]
        fn wtf8_roundtrips_any_units(u in proptest::collection::vec(any::<u16>(), 0..64)) {
            prop_assert_eq!(from_wtf8(&to_wtf8(&u)), Some(u));
        }
    }
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test -p rf-core --lib winpath`
Expected: FAIL, 컴파일 오류 `error[E0425]: cannot find …` (`MAX_EXTENDED_PATH`, `to_extended_length` 등)

- [ ] **Step 4: Interfaces의 다섯 항목을 `winpath.rs`의 테스트 모듈 위에 구현한다**

모듈 문서 주석: "Windows path-prefix rules as pure functions over UTF-16 code units; compiled on every OS so Linux CI tests them." 규칙(테스트가 정하지 않는 부분):
- `to_extended_length`: (1) 앞 네 단위가 `구분자 구분자 (? 또는 .) 구분자`면(구분자는 `\`와 `/`) 입력을 그대로 복사해 돌려준다. (2) 그 밖에는 `/`를 모두 `\`로 바꾼 뒤, `X:\…`(X는 ASCII 글자)이면 `\\?\`를 앞에 붙이고, `\\server…`(세 번째 단위가 `\`가 아님)이면 앞의 `\\`를 `\\?\UNC\`로 바꾼다. (3) 나머지는 `None`. 길이 제한은 검사하지 않는다(`MAX_EXTENDED_PATH`는 호출자가 쓰는 상한이다).
- `strip_extended_length`: `\\?\UNC\`(ASCII 대소문자 무시)로 시작하면 `\\` + 나머지, `\\?\X:\`로 시작하면 `X:\…`, 그 밖에는 그대로.
- `to_wtf8`: `char::decode_utf16`. 정상 문자는 UTF-8, 짝 없는 서로게이트 `u`는 3바이트 `0xE0 | u >> 12`, `0x80 | (u >> 6 & 0x3F)`, `0x80 | (u & 0x3F)`.
- `from_wtf8`: 1~4바이트 UTF-8 디코더인데 3바이트 서로게이트(U+D800~U+DFFF)를 단위 하나로 받아들인다. 잘린 시퀀스, 잘못된 연속 바이트, 과잉 길이(overlong), U+10FFFF 초과는 `None`. 4바이트 문자는 서로게이트 쌍으로 낸다.
- `u16`→`u8` 변환은 `u8::try_from`으로 한다(clippy `cast_possible_truncation`).

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test -p rf-core --lib winpath`
Expected: `test result: ok. 7 passed`

- [ ] **Step 6: 표준 검사를 돌린다**

- [ ] **Step 7: 커밋한다**

```bash
git add Cargo.lock crates/rf-core
git commit -m "feat(rf-core): add Windows path prefix and WTF-8 helpers"
```

---

### Task 4: VPath 로컬 경로와 무손실 문자열 형식

**Files:**
- Create: `crates/rf-core/src/error.rs` (이 태스크에서는 `PathError`만), `crates/rf-core/src/vpath/mod.rs`, `crates/rf-core/src/vpath/scheme.rs`, `crates/rf-core/src/vpath/escape.rs` (모듈 안 테스트 포함), `crates/rf-core/src/vpath/local.rs`
- Modify: `crates/rf-core/src/lib.rs`, `crates/rf-core/Cargo.toml` (cargo add)
- Test: `crates/rf-core/tests/vpath_local.rs`

**Interfaces:**
- Consumes: `winpath::{strip_extended_length, to_wtf8, from_wtf8}` (Task 3)
- Produces:
  - `rf_core::error::PathError` (아래 정의), 재수출 `rf_core::PathError`
  - `rf_core::vpath::Scheme` (`Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord`, `Display` = `name()`): `Local, Archive, Remote, Shell, Collection, Search`, `pub const ALL: &[Scheme]`(이 순서), `pub const fn name(self) -> &'static str` (`"local"`, `"archive"`, `"remote"`, `"shell"`, `"collection"`, `"search"`), `pub fn from_name(name: &str) -> Option<Scheme>` (ASCII 대소문자 무시). 일부러 `non_exhaustive`가 아니다. 스킴이 늘면 워크스페이스의 모든 `match`가 컴파일 오류로 알려 준다.
  - `rf_core::vpath::VPath` (`Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord`, `Display`, `FromStr<Err = PathError>`): `local`, `parse`, `to_uri`, `scheme`, `as_local`
  - 크레이트 내부: `vpath::escape::{escape_bytes(bytes: &[u8], extra: &[char]) -> String, unescape_bytes(s: &str) -> Result<Vec<u8>, PathError>, escape_os(s: &OsStr, extra: &[char]) -> String, unescape_os(s: &str) -> Result<OsString, PathError>}` (`pub(crate)`), `vpath::local::{normalize(path: &Path) -> Result<PathBuf, PathError>, check_name(name: &OsStr) -> Result<(), PathError>}` (`pub(super)`)

- [ ] **Step 1: 의존성을 추가한다**

Run: `cargo add -p rf-core thiserror`
Expected: `Adding thiserror v2.` 로 시작하는 줄

- [ ] **Step 2: 실패하는 테스트를 쓴다**

lib.rs에 `pub mod vpath;`, `vpath/mod.rs`에 `mod escape;` 한 줄만 두고, `vpath/escape.rs`에는 아래 테스트 모듈만 둔다.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn escape_keeps_text_and_escapes_percent_controls_and_invalid_bytes() {
        assert_eq!(escape_bytes("한글 a.txt".as_bytes(), &[]), "한글 a.txt");
        assert_eq!(escape_bytes(b"100%.txt", &[]), "100%25.txt");
        assert_eq!(escape_bytes(b"a\nb\x7f", &[]), "a%0Ab%7F");
        assert_eq!(escape_bytes(b"\xff\xfe", &[]), "%FF%FE");
        assert_eq!(escape_bytes(b"a/b", &['/']), "a%2Fb");
    }

    #[test]
    fn unescape_rejects_bad_escapes() {
        assert_eq!(unescape_bytes("a%2"), Err(PathError::InvalidEscape("a%2".into())));
        assert_eq!(unescape_bytes("a%zz"), Err(PathError::InvalidEscape("a%zz".into())));
        assert_eq!(unescape_bytes("a%+1"), Err(PathError::InvalidEscape("a%+1".into())));
        assert_eq!(unescape_bytes("%41%e4"), Ok(vec![b'A', 0xE4]));
    }

    proptest! {
        #[test]
        fn escape_roundtrips_any_bytes(b in proptest::collection::vec(any::<u8>(), 0..64)) {
            prop_assert_eq!(unescape_bytes(&escape_bytes(&b, &['/'])).unwrap(), b);
        }
    }
}
```

`crates/rf-core/tests/vpath_local.rs`:

```rust
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use rf_core::{PathError, Scheme, VPath};

/// Absolute host path: `C:\a\b` on Windows, `/a/b` elsewhere.
fn abs(parts: &[&str]) -> PathBuf {
    let mut p = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
    for part in parts {
        p.push(part);
    }
    p
}

fn local(parts: &[&str]) -> VPath {
    VPath::local(abs(parts)).unwrap()
}

#[test]
fn scheme_names_roundtrip() {
    let names: Vec<&str> = Scheme::ALL.iter().map(|s| s.name()).collect();
    assert_eq!(names, ["local", "archive", "remote", "shell", "collection", "search"]);
    for &s in Scheme::ALL {
        assert_eq!(Scheme::from_name(s.name()), Some(s));
        assert_eq!(Scheme::from_name(&s.name().to_uppercase()), Some(s));
        assert_eq!(s.to_string(), s.name());
    }
    assert_eq!(Scheme::from_name("sftp"), None);
}

#[test]
fn local_rejects_empty_relative_and_nul() {
    assert_eq!(VPath::local(""), Err(PathError::Empty));
    assert_eq!(VPath::parse(""), Err(PathError::Empty));
    assert_eq!(VPath::local("rel/x"), Err(PathError::NotAbsolute("rel/x".into())));
    assert_eq!(VPath::local(abs(&["a\0b"])), Err(PathError::Nul));
}

#[test]
fn local_normalizes_dot_segments() {
    let p = VPath::local(abs(&["a", ".", "b", "..", "c"])).unwrap();
    assert_eq!(p.scheme(), Scheme::Local);
    assert_eq!(p.as_local(), Some(abs(&["a", "c"]).as_path()));
    assert_eq!(local(&["..", ".."]).as_local(), Some(abs(&[]).as_path()));
}

#[test]
fn uri_roundtrip_escapes_percent_and_controls() {
    let p = local(&["dir", "100%.txt"]);
    assert!(p.to_uri().starts_with("local:"));
    assert!(p.to_uri().ends_with("100%25.txt"));
    assert_eq!(VPath::parse(&p.to_uri()), Ok(p.clone()));
    assert_eq!(VPath::parse(&p.to_string()), Ok(p.clone()));
    assert_eq!(p.to_string().parse::<VPath>(), Ok(p));
    let nl = local(&["a\nb"]);
    assert!(nl.to_uri().ends_with("a%0Ab"));
    assert_eq!(VPath::parse(&nl.to_uri()), Ok(nl));
}

#[test]
fn native_input_is_never_unescaped() {
    let p = VPath::parse(abs(&["a%41"]).to_str().unwrap()).unwrap();
    assert_eq!(p.as_local().and_then(Path::file_name), Some(OsStr::new("a%41")));
    assert!(p.to_uri().ends_with("a%2541"));
}

#[test]
fn unicode_is_kept_exactly() {
    let nfc = local(&["한글"]);
    let nfd = local(&["\u{1112}\u{1161}\u{11AB}\u{1100}\u{1173}\u{11AF}"]);
    assert_ne!(nfc, nfd);
    assert!(nfc.to_uri().ends_with("한글"));
    assert_eq!(VPath::parse(&nfd.to_uri()), Ok(nfd));
    let emoji = local(&["🦝.png"]);
    assert_eq!(VPath::parse(&emoji.to_string()), Ok(emoji));
}

#[test]
fn parse_rejects_unknown_scheme_and_bad_escape() {
    assert_eq!(VPath::parse("sftp://host/x"), Err(PathError::UnknownScheme("sftp".into())));
    assert_eq!(VPath::parse("local:%zz"), Err(PathError::InvalidEscape("%zz".into())));
    assert_eq!(VPath::parse("local:relative"), Err(PathError::NotAbsolute("relative".into())));
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    #[test]
    fn non_utf8_bytes_roundtrip() {
        let raw = Path::new(OsStr::from_bytes(b"/tmp/\xff\xfe.bin"));
        let p = VPath::local(raw).unwrap();
        assert_eq!(p.to_uri(), "local:/tmp/%FF%FE.bin");
        assert_eq!(VPath::parse(&p.to_uri()), Ok(p.clone()));
        assert_eq!(p.as_local(), Some(raw));
        assert!(p.to_string().contains('\u{FFFD}'));
    }

    #[test]
    fn repeated_slashes_collapse() {
        assert_eq!(VPath::local("//a///b/").unwrap().as_local(), Some(Path::new("/a/b")));
    }

    proptest::proptest! {
        #[test]
        fn any_byte_names_roundtrip(
            names in proptest::collection::vec(proptest::collection::vec(1u8..=255, 1..12), 0..5),
        ) {
            let mut raw = PathBuf::from("/");
            for n in names.iter().filter(|n| !n.contains(&b'/') && n.as_slice() != b"." && n.as_slice() != b"..") {
                raw.push(OsStr::from_bytes(n));
            }
            let p = VPath::local(&raw).unwrap();
            proptest::prop_assert_eq!(p.as_local(), Some(raw.as_path()));
            proptest::prop_assert_eq!(VPath::parse(&p.to_uri()), Ok(p));
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    fn lp(s: &str) -> Option<PathBuf> {
        VPath::local(s).ok().and_then(|p| p.as_local().map(Path::to_path_buf))
    }

    #[test]
    fn drive_letters_uppercase_and_separators_unify() {
        assert_eq!(lp(r"c:\Users\..\x"), Some(PathBuf::from(r"C:\x")));
        assert_eq!(lp("C:/a/b"), Some(PathBuf::from(r"C:\a\b")));
    }

    #[test]
    fn verbatim_prefix_is_stripped() {
        assert_eq!(lp(r"\\?\C:\x"), Some(PathBuf::from(r"C:\x")));
        assert_eq!(lp(r"\\?\UNC\srv\share\x"), Some(PathBuf::from(r"\\srv\share\x")));
    }

    #[test]
    fn rejects_relative_device_and_stream_forms() {
        assert_eq!(VPath::local(r"C:x"), Err(PathError::NotAbsolute("C:x".into())));
        assert_eq!(VPath::local(r"\x"), Err(PathError::NotAbsolute(r"\x".into())));
        assert_eq!(VPath::local(r"\\.\COM1"), Err(PathError::Unsupported(r"\\.\COM1".into())));
        assert_eq!(VPath::local(r"C:\a\b:c"), Err(PathError::InvalidName("b:c".into())));
    }

    #[test]
    fn lone_surrogate_roundtrips() {
        let mut w: Vec<u16> = r"C:\".encode_utf16().collect();
        w.push(0xD800);
        let p = VPath::local(PathBuf::from(OsString::from_wide(&w))).unwrap();
        assert_eq!(p.to_uri(), r"local:C:\%ED%A0%80");
        assert_eq!(VPath::parse(&p.to_uri()), Ok(p.clone()));
        let back: Vec<u16> = p.as_local().unwrap().as_os_str().encode_wide().collect();
        assert_eq!(back, w);
    }

    proptest::proptest! {
        #[test]
        fn any_unit_names_roundtrip(
            names in proptest::collection::vec(proptest::collection::vec(1u16..=u16::MAX, 1..12), 0..5),
        ) {
            let bad = |u: &u16| [u16::from(b'/'), u16::from(b'\\'), u16::from(b':')].contains(u);
            let mut raw = PathBuf::from(r"C:\");
            for n in names.iter().filter(|n| !n.iter().any(bad) && n.iter().any(|u| *u != u16::from(b'.'))) {
                raw.push(OsString::from_wide(n));
            }
            let p = VPath::local(&raw).unwrap();
            proptest::prop_assert_eq!(p.as_local(), Some(raw.as_path()));
            proptest::prop_assert_eq!(VPath::parse(&p.to_uri()), Ok(p));
        }
    }
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test -p rf-core --test vpath_local`
Expected: FAIL, `error[E0432]: unresolved imports `rf_core::PathError`, `rf_core::Scheme`, `rf_core::VPath``
Run: `cargo test -p rf-core --lib escape`
Expected: FAIL, 컴파일 오류(`PathError`와 `escape_bytes`를 찾지 못함)

- [ ] **Step 4: 구현한다**

`error.rs`에 아래 정의를 그대로 쓴다(메시지 문자열 고정). lib.rs에 `pub mod error;`, `pub use error::PathError;`, `pub use vpath::{Scheme, VPath};`를 추가한다.

```rust
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PathError {
    #[error("path is empty")]
    Empty,
    #[error("path contains a NUL character")]
    Nul,
    #[error("path is not absolute: {0}")]
    NotAbsolute(String),
    #[error("unknown path scheme `{0}`")]
    UnknownScheme(String),
    #[error("invalid escape sequence in `{0}`")]
    InvalidEscape(String),
    #[error("invalid name `{0}`")]
    InvalidName(String),
    #[error("unsupported path form: {0}")]
    Unsupported(String),
}
```

`String` 인자는 문제가 된 입력의 손실 변환(`to_string_lossy`)이다.

`vpath/scheme.rs`: Interfaces의 `Scheme`.

`vpath/escape.rs` (테스트 모듈 위):
- `escape_bytes`: `<[u8]>::utf8_chunks()`로 돈다. 유효한 문자는 그대로 쓰되 `%`, ASCII 제어 문자(U+0000~U+001F, U+007F), `extra`에 든 문자는 UTF-8 바이트마다 `%XX`(대문자 16진수)로 쓴다. 잘못된 바이트는 하나씩 `%XX`.
- `unescape_bytes`: `%` 뒤에 ASCII 16진수 두 자리가 오면 그 바이트, 아니면 `PathError::InvalidEscape(s 전체)`. 나머지 바이트는 그대로.
- `escape_os`: `#[cfg(unix)]` `OsStrExt::as_bytes`, `#[cfg(windows)]` `encode_wide` → `winpath::to_wtf8` 후 `escape_bytes`.
- `unescape_os`: `unescape_bytes` 후 `#[cfg(unix)]` `OsStringExt::from_vec`, `#[cfg(windows)]` `winpath::from_wtf8`(`None`이면 `InvalidEscape(s)`) → `OsString::from_wide`. `unsafe`인 `from_encoded_bytes_unchecked`는 쓰지 않는다.

`vpath/local.rs`:
- `check_name(name)`: 비었거나 `.`·`..`이거나 `/`·NUL을 포함하면(Windows는 `\`·`:`도) `InvalidName(lossy)`. Windows의 `:`는 대체 데이터 스트림(`a:b`)을 뜻하므로 막는다.
- `normalize` `#[cfg(unix)]`: 빈 경로 → `Empty`, NUL 포함(`as_encoded_bytes()`에 0) → `Nul`, `has_root()`가 아니면 `NotAbsolute(입력)`. `/`에서 시작해 `Component::Normal`은 push, `ParentDir`는 pop(루트에서 멈춤), `CurDir`는 무시한다.
- `normalize` `#[cfg(windows)]`: 빈 경로·NUL 검사 → `encode_wide` → `winpath::strip_extended_length` → `OsString::from_wide` → `components()`. 첫 구성요소는 `Prefix`여야 한다: `Disk(d)`는 대문자 `D:`, `UNC(server, share)`는 `\\server\share`(`OsString`에 이어 붙여 무손실 조립), 남은 `Verbatim*`(예: `\\?\Volume{…}`)은 접두사 원문 그대로, `DeviceNS`는 `Unsupported(입력)`. Prefix가 아니거나 다음이 `RootDir`가 아니면 `NotAbsolute(입력)`. 루트는 접두사 + `\`이고, 나머지는 `Normal`마다 `check_name` 후 push, `ParentDir`는 pop(루트에서 멈춤), `CurDir`는 무시.

`vpath/mod.rs`:
- `pub struct VPath { repr: Repr }`, 비공개 `enum Repr { Local(PathBuf) }`(Task 6에서 변형 추가). 비교·해시는 derive.
- `local(path)`: `local::normalize`.
- `parse(input)`: 빈 문자열 → `Empty`. 비공개 `fn split_scheme(input: &str) -> Option<(&str, &str)>`는 첫 `:` 앞이 두 글자 이상이고 `[A-Za-z][A-Za-z0-9+.-]*`일 때만 스킴으로 본다(한 글자는 Windows 드라이브). 스킴이 없으면 입력을 그대로 `VPath::local`에 넘긴다(unescape 안 함). 모르는 이름은 `UnknownScheme(입력한 이름)`. `local`이면 `VPath::local(escape::unescape_os(rest)?)`. 다른 알려진 스킴은 Task 6 전까지 `Unsupported(format!("{name} paths are not implemented"))`.
- `to_uri()`: `"local:"` + `escape_os(path, &[])`. `Display`: 로컬은 `Path::display()`(사람용, 손실 가능). `FromStr`: `parse`.
- `scheme()`, `as_local()`.

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test -p rf-core --lib escape`
Expected: `test result: ok. 3 passed`
Run: `cargo test -p rf-core --test vpath_local`
Expected: Linux·macOS `test result: ok. 10 passed`, Windows `test result: ok. 12 passed`

- [ ] **Step 6: 표준 검사와 교차 검사를 돌린다** (Global Constraints, `cfg(windows)` 코드가 들어오므로 교차 검사 필수)

- [ ] **Step 7: 커밋한다**

```bash
git add Cargo.lock crates/rf-core
git commit -m "feat(rf-core): add VPath with lossless local paths"
```

---

### Task 5: VPath 탐색과 확장 길이 형식

**Files:**
- Modify: `crates/rf-core/src/vpath/mod.rs`, `crates/rf-core/src/vpath/local.rs`
- Test: `crates/rf-core/tests/vpath_nav.rs`

**Interfaces:**
- Consumes: `VPath::local`, `local::check_name` (Task 4), `winpath::to_extended_length` (Task 3)
- Produces:
  - `pub fn file_name(&self) -> Option<&OsStr>`: 루트면 `None`
  - `pub fn parent(&self) -> Option<VPath>`: 루트면 `None`
  - `pub fn join(&self, name: impl AsRef<OsStr>) -> Result<VPath, PathError>`: 이름 하나만 받는다(구분자 포함 불가)
  - `pub fn starts_with(&self, base: &VPath) -> bool`: 구성요소 단위, 같은 경로면 `true`
  - `pub fn to_extended_length(&self) -> Option<PathBuf>`: Windows는 항상 `\\?\` 형식, 다른 OS는 로컬 경로 그대로, 비로컬은 `None`. rf-platform은 Windows 파일 API에 넘기는 모든 경로를 이것으로 만든다(긴 경로, 예약 이름, 끝 점·공백 이름이 글자 그대로 전달됨). 셸 API(ShellExecute 등)에는 `as_local()`을 쓴다.
  - 크레이트 내부: `vpath::local::extended_length(p: &Path) -> PathBuf` (`pub(super)`)

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`crates/rf-core/tests/vpath_nav.rs`:

```rust
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use rf_core::{PathError, VPath};

fn abs(parts: &[&str]) -> PathBuf {
    let mut p = PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" });
    for part in parts {
        p.push(part);
    }
    p
}

fn local(parts: &[&str]) -> VPath {
    VPath::local(abs(parts)).unwrap()
}

#[test]
fn parent_file_name_join() {
    let ab = local(&["a", "b"]);
    assert_eq!(ab.file_name(), Some(OsStr::new("b")));
    assert_eq!(ab.parent(), Some(local(&["a"])));
    assert_eq!(local(&["a"]).parent(), Some(local(&[])));
    assert_eq!(local(&[]).parent(), None);
    assert_eq!(local(&[]).file_name(), None);
    assert_eq!(local(&["a"]).join("b"), Ok(ab));
}

#[test]
fn join_rejects_non_names() {
    let base = local(&["a"]);
    for bad in ["", ".", "..", "x/y", "nul\0"] {
        assert_eq!(base.join(bad), Err(PathError::InvalidName(bad.into())), "{bad:?}");
    }
}

#[test]
fn starts_with_is_component_wise() {
    let ab = local(&["a", "b"]);
    assert!(local(&["a", "b", "c"]).starts_with(&ab));
    assert!(ab.starts_with(&ab));
    assert!(!local(&["a", "bc"]).starts_with(&ab));
    assert!(!local(&["a"]).starts_with(&ab));
}

#[cfg(unix)]
mod unix {
    use super::*;

    #[test]
    fn backslash_is_an_ordinary_character() {
        let p = VPath::local("/a").unwrap().join(r"b\c").unwrap();
        assert_eq!(p.as_local(), Some(Path::new(r"/a/b\c")));
        assert_eq!(p.to_extended_length(), Some(PathBuf::from(r"/a/b\c")));
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn join_rejects_backslash_and_colon() {
        let base = VPath::local(r"C:\a").unwrap();
        assert_eq!(base.join(r"b\c"), Err(PathError::InvalidName(r"b\c".into())));
        assert_eq!(base.join("b:c"), Err(PathError::InvalidName("b:c".into())));
    }

    #[test]
    fn unc_roots() {
        let root = VPath::local(r"\\srv\share\x").unwrap().parent().unwrap();
        assert_eq!(root.as_local(), Some(Path::new(r"\\srv\share\")));
        assert_eq!(root.parent(), None);
        assert_eq!(VPath::local(r"C:\").unwrap().parent(), None);
    }

    #[test]
    fn extended_length_form() {
        let p = VPath::local(r"C:\a").unwrap();
        assert_eq!(p.to_extended_length(), Some(PathBuf::from(r"\\?\C:\a")));
        let u = VPath::local(r"\\srv\share\x").unwrap();
        assert_eq!(u.to_extended_length(), Some(PathBuf::from(r"\\?\UNC\srv\share\x")));
        let long = (0..40).fold(VPath::local(r"C:\").unwrap(), |p, _| p.join("abcdefghij").unwrap());
        assert!(long.as_local().unwrap().as_os_str().len() > 260);
        let ext = long.to_extended_length().unwrap();
        assert!(ext.to_string_lossy().starts_with(r"\\?\C:\abcdefghij\"));
    }

    #[test]
    fn reserved_and_trailing_dot_names_stay_literal() {
        let con = VPath::local(r"C:\a\CON").unwrap();
        assert_eq!(con.to_extended_length(), Some(PathBuf::from(r"\\?\C:\a\CON")));
        let dot = VPath::local(r"C:\a").unwrap().join("foo.").unwrap();
        assert_eq!(dot.file_name(), Some(OsStr::new("foo.")));
        assert_eq!(dot.to_extended_length(), Some(PathBuf::from(r"\\?\C:\a\foo.")));
    }
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test -p rf-core --test vpath_nav`
Expected: FAIL, `error[E0599]: no method named … found for struct `VPath`` (`file_name`, `parent`, `join`, `starts_with`)

- [ ] **Step 3: Interfaces의 메서드를 구현한다**

- `file_name`·`parent`: `Path::file_name`·`Path::parent`. 저장된 경로가 이미 정규화돼 있으므로 `parent` 결과는 다시 정규화하지 않고 감싼다.
- `join`: `local::check_name(name)?` 후 `PathBuf::join`.
- `starts_with`: `Path::starts_with`(구성요소 비교).
- `local::extended_length` `#[cfg(windows)]`: `encode_wide` → `winpath::to_extended_length` → `OsString::from_wide`(`None`이면 입력 그대로. `VPath`는 항상 절대 경로라 실제로는 생기지 않는다). `#[cfg(not(windows))]`: 입력 복사.

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test -p rf-core --test vpath_nav`
Expected: Linux·macOS `test result: ok. 4 passed`, Windows `test result: ok. 7 passed`

- [ ] **Step 5: 표준 검사와 교차 검사를 돌린다**

- [ ] **Step 6: 커밋한다**

```bash
git add crates/rf-core
git commit -m "feat(rf-core): add VPath navigation and extended-length form"
```

---

### Task 6: 비로컬 스킴 경로와 serde

**Files:**
- Modify: `crates/rf-core/src/vpath/mod.rs`, `crates/rf-core/Cargo.toml` (cargo add)
- Test: `crates/rf-core/tests/vpath_scheme.rs`

**Interfaces:**
- Consumes: Task 4·5의 `VPath`, `escape::{escape_bytes, unescape_bytes}`
- Produces:
  - `pub fn new(scheme: Scheme, authority: &str, segments: &[&str]) -> Result<VPath, PathError>`: 비로컬 경로 `scheme://authority/seg/…`. authority와 segment의 의미는 각 공급자 계획이 정한다. 예: shell은 `shell://thispc`, remote는 연결 프로필 ID, archive는 바깥 파일 `VPath`의 `to_uri()`를 authority로 둔다(중첩 가능).
  - `pub fn authority(&self) -> Option<&str>`, `pub fn segments(&self) -> Option<&[String]>`: 로컬이면 `None`
  - `impl serde::Serialize for VPath`(문자열 `to_uri()`), `impl<'de> serde::Deserialize<'de> for VPath`(`parse`, 실패는 `serde::de::Error::custom`). 저널·원장·설정·메타 DB는 이 문자열 형식으로 저장한다.
  - 기존 메서드의 비로컬 동작: `file_name` = 마지막 segment, `parent` = 마지막 segment 제거(없으면 `None`), `join` = UTF-8 이름 하나 추가, `starts_with` = 같은 스킴·같은 authority·segment 접두, 로컬과 비로컬 사이는 항상 `false`, `as_local`·`to_extended_length` = `None`, `Display` = `to_uri()`

- [ ] **Step 1: 의존성을 추가한다**

Run: `cargo add -p rf-core serde` 그리고 `cargo add -p rf-core --dev serde_json`
Expected: `Adding serde v1.` 과 `Adding serde_json v1.` 로 시작하는 줄

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`crates/rf-core/tests/vpath_scheme.rs`:

```rust
use std::ffi::OsStr;
use std::path::PathBuf;

use rf_core::{PathError, Scheme, VPath};

fn local_root() -> VPath {
    VPath::local(PathBuf::from(if cfg!(windows) { r"C:\" } else { "/" })).unwrap()
}

#[test]
fn non_local_uri_and_navigation() {
    let p = VPath::new(Scheme::Remote, "prof 1", &["dir", "a b", "50%"]).unwrap();
    assert_eq!(p.to_uri(), "remote://prof 1/dir/a b/50%25");
    assert_eq!(p.to_string(), p.to_uri());
    assert_eq!(VPath::parse(&p.to_uri()), Ok(p.clone()));
    assert_eq!(p.scheme(), Scheme::Remote);
    assert_eq!(p.authority(), Some("prof 1"));
    let segs: Vec<String> = ["dir", "a b", "50%"].map(String::from).to_vec();
    assert_eq!(p.segments(), Some(segs.as_slice()));
    assert_eq!(p.file_name(), Some(OsStr::new("50%")));
    assert_eq!(p.parent().unwrap().to_uri(), "remote://prof 1/dir/a b");
    assert_eq!(p.as_local(), None);
    assert_eq!(p.to_extended_length(), None);
    assert_eq!(local_root().authority(), None);
    assert_eq!(local_root().segments(), None);
}

#[test]
fn non_local_root_and_join() {
    let root = VPath::new(Scheme::Shell, "thispc", &[]).unwrap();
    assert_eq!(root.to_uri(), "shell://thispc");
    assert_eq!(root.parent(), None);
    assert_eq!(root.file_name(), None);
    assert_eq!(root.join("C:"), VPath::new(Scheme::Shell, "thispc", &["C:"]));
    assert_eq!(root.join(".."), Err(PathError::InvalidName("..".into())));
    assert_eq!(VPath::parse("shell://thispc/"), Ok(root));
}

#[test]
fn archive_authority_nests_a_vpath() {
    let outer = local_root().join("x.zip").unwrap();
    let arc = VPath::new(Scheme::Archive, &outer.to_uri(), &["inner", "a.txt"]).unwrap();
    let escaped = outer.to_uri().replace('%', "%25").replace('/', "%2F");
    assert_eq!(arc.to_uri(), format!("archive://{escaped}/inner/a.txt"));
    assert_eq!(VPath::parse(&arc.to_uri()), Ok(arc.clone()));
    assert_eq!(arc.authority().map(VPath::parse), Some(Ok(outer)));
}

#[test]
fn new_and_parse_reject_bad_input() {
    assert_eq!(
        VPath::new(Scheme::Local, "", &[]),
        Err(PathError::Unsupported("use VPath::local for local paths".into()))
    );
    assert_eq!(VPath::new(Scheme::Remote, "p", &[".."]), Err(PathError::InvalidName("..".into())));
    assert_eq!(VPath::new(Scheme::Remote, "p", &["a/b"]), Err(PathError::InvalidName("a/b".into())));
    assert_eq!(VPath::new(Scheme::Remote, "p\0", &[]), Err(PathError::Nul));
    assert_eq!(VPath::parse("remote:/x"), Err(PathError::Unsupported("expected `remote://`".into())));
}

#[test]
fn starts_with_never_crosses_schemes() {
    let a = VPath::new(Scheme::Remote, "p1", &["a"]).unwrap();
    let ab = a.join("b").unwrap();
    assert!(ab.starts_with(&a));
    assert!(!ab.starts_with(&VPath::new(Scheme::Remote, "p2", &["a"]).unwrap()));
    assert!(!ab.starts_with(&VPath::new(Scheme::Archive, "p1", &["a"]).unwrap()));
    assert!(!ab.starts_with(&local_root()));
    assert!(!local_root().starts_with(&a));
}

#[test]
fn serde_uses_uri_string() {
    let p = VPath::new(Scheme::Remote, "p1", &["a b"]).unwrap();
    let json = serde_json::to_string(&p).unwrap();
    assert_eq!(json, "\"remote://p1/a b\"");
    assert_eq!(serde_json::from_str::<VPath>(&json).unwrap(), p);
    assert!(serde_json::from_str::<VPath>("\"sftp://x\"").is_err());
}

proptest::proptest! {
    #[test]
    fn non_local_roundtrip(
        auth in "[^\\x00]{0,12}",
        segs in proptest::collection::vec("[^/\\x00]{1,12}", 0..5),
    ) {
        let segs: Vec<&str> = segs.iter().map(String::as_str).filter(|s| *s != "." && *s != "..").collect();
        let p = VPath::new(Scheme::Remote, &auth, &segs).unwrap();
        proptest::prop_assert_eq!(VPath::parse(&p.to_uri()), Ok(p));
    }
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test -p rf-core --test vpath_scheme`
Expected: FAIL, `error[E0599]: no function or associated item named `new` found for struct `VPath``

- [ ] **Step 4: 구현한다**

- `Repr`에 `Other { scheme: Scheme, authority: String, segments: Vec<String> }`를 더하고 Task 4·5의 모든 `match`를 Interfaces의 비로컬 동작으로 넓힌다.
- `new`: `Scheme::Local`이면 `Unsupported("use VPath::local for local paths")`. authority에 NUL이 있으면 `Nul`(그 밖의 문자는 빈 문자열과 `/`까지 모두 허용). 비공개 `fn check_segment(name: &str) -> Result<(), PathError>`: 비었거나 `.`·`..`이거나 `/`·NUL 포함이면 `InvalidName(name)`.
- `to_uri`: `format!("{}://{}", scheme.name(), escape_bytes(authority, &['/']))`에 segment마다 `/` + `escape_bytes(seg, &['/'])`. 끝에 `/`를 붙이지 않는다.
- `parse`의 비로컬 갈래: `rest`가 `//`로 시작하지 않으면 `Unsupported(format!("expected `{}://`", scheme.name()))`. 나머지를 `/`로 나눠 첫 조각은 authority, 이후 빈 조각은 건너뛰고 각 조각을 `unescape_bytes` 후 UTF-8 `String`으로(실패하면 `InvalidEscape(조각)`) 바꾼 뒤 `VPath::new`.
- `join`의 비로컬 갈래: `name.to_str()`이 `None`이면 `InvalidName(lossy)`, 그 뒤 `check_segment`.
- serde 두 impl은 Interfaces대로(serde `derive` 기능은 쓰지 않는다).

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test -p rf-core --test vpath_scheme`
Expected: `test result: ok. 7 passed`
Run: `cargo test -p rf-core`
Expected: 모든 `test result:` 줄이 `ok.`이고 `FAILED`가 없음

- [ ] **Step 6: 표준 검사와 교차 검사를 돌린다**

- [ ] **Step 7: 커밋한다**

```bash
git add Cargo.lock crates/rf-core
git commit -m "feat(rf-core): add non-local VPath schemes and serde"
```

---

### Task 7: 공용 오류 타입

**Files:**
- Modify: `crates/rf-core/src/error.rs`, `crates/rf-core/src/lib.rs`
- Test: `crates/rf-core/tests/error.rs`

**Interfaces:**
- Consumes: `VPath`, `PathError` (Task 4)
- Produces:
  - `pub type Result<T, E = Error> = std::result::Result<T, E>;`
  - `pub enum Error` (아래 정의, `Send + Sync + 'static`), `impl From<PathError> for Error`
  - `pub fn io(op: &'static str, path: VPath, source: std::io::Error) -> Error`: `op`는 실패한 동작 이름(예: `"open"`, `"create_dir_all"`)
  - `pub fn kind(&self) -> ErrorKind`
  - `pub enum ErrorKind { NotFound, PermissionDenied, AlreadyExists, InvalidPath, Unsupported, Cancelled, Other }` (`Clone, Copy, Debug, PartialEq, Eq, Hash`, `non_exhaustive`). rf-jobs의 재시도·건너뛰기(XFER-09)와 충돌 판단, UI 문구 선택이 이 값으로 분기한다.
  - 재수출: `rf_core::{Error, ErrorKind, PathError, Result}`

- [ ] **Step 1: 실패하는 테스트를 쓴다**

`crates/rf-core/tests/error.rs`:

```rust
use std::io;

use rf_core::{Error, ErrorKind, PathError, Result, VPath};

fn here() -> VPath {
    VPath::local(std::env::temp_dir()).unwrap()
}

#[test]
fn io_errors_classify_by_kind() {
    let cases = [
        (io::ErrorKind::NotFound, ErrorKind::NotFound),
        (io::ErrorKind::PermissionDenied, ErrorKind::PermissionDenied),
        (io::ErrorKind::AlreadyExists, ErrorKind::AlreadyExists),
        (io::ErrorKind::Unsupported, ErrorKind::Unsupported),
        (io::ErrorKind::TimedOut, ErrorKind::Other),
    ];
    for (io_kind, kind) in cases {
        assert_eq!(Error::io("open", here(), io::Error::from(io_kind)).kind(), kind);
    }
}

#[test]
fn display_names_operation_and_path() {
    let e = Error::io("open", here(), io::Error::from(io::ErrorKind::NotFound));
    let msg = e.to_string();
    assert!(msg.starts_with(&format!("open failed for {}: ", here())), "{msg}");
    assert!(std::error::Error::source(&e).is_some());
}

#[test]
fn other_variants_classify() {
    assert_eq!(Error::from(PathError::Empty).kind(), ErrorKind::InvalidPath);
    assert_eq!(Error::Cancelled.kind(), ErrorKind::Cancelled);
    assert_eq!(Error::Unsupported("x".into()).kind(), ErrorKind::Unsupported);
    assert_eq!(Error::MissingEnv("HOME").kind(), ErrorKind::Other);
    assert_eq!(Error::from(PathError::Empty).to_string(), "path is empty");
}

#[test]
fn question_mark_converts_path_errors() {
    fn f() -> Result<VPath> {
        Ok(VPath::parse("relative")?)
    }
    assert!(matches!(f(), Err(Error::Path(PathError::NotAbsolute(_)))));
}

#[test]
fn shared_types_cross_threads() {
    fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<Error>();
    assert_send_sync::<VPath>();
}
```

- [ ] **Step 2: 실패를 확인한다**

Run: `cargo test -p rf-core --test error`
Expected: FAIL, `error[E0432]: unresolved imports `rf_core::Error`, `rf_core::ErrorKind`, `rf_core::Result``

- [ ] **Step 3: `error.rs`에 아래 정의와 두 메서드를 추가하고 lib.rs 재수출을 `pub use error::{Error, ErrorKind, PathError, Result};`로 바꾼다**

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("{op} failed for {path}: {source}")]
    Io { op: &'static str, path: VPath, #[source] source: std::io::Error },
    #[error(transparent)]
    Path(#[from] PathError),
    #[error("environment variable {0} is not set; cannot locate the app data folders")]
    MissingEnv(&'static str),
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("operation cancelled")]
    Cancelled,
}
```

`kind()` 대응: `Io`는 `source.kind()`가 `NotFound`·`PermissionDenied`·`AlreadyExists`·`Unsupported`면 같은 이름, 그 밖에는 `Other`. `Path` → `InvalidPath`, `Unsupported` → `Unsupported`, `Cancelled` → `Cancelled`, `MissingEnv` → `Other`. (`io::ErrorKind::InvalidFilename`은 1.87에서 안정화돼 MSRV 1.85에서 쓸 수 없다.)

- [ ] **Step 4: 통과를 확인한다**

Run: `cargo test -p rf-core --test error`
Expected: `test result: ok. 5 passed`

- [ ] **Step 5: 표준 검사를 돌린다**

- [ ] **Step 6: 커밋한다**

```bash
git add crates/rf-core
git commit -m "feat(rf-core): add shared Error type"
```

---

### Task 8: Entry 메타데이터 모델

**Files:**
- Create: `crates/rf-core/src/entry.rs`
- Modify: `crates/rf-core/src/lib.rs`, `crates/rf-core/Cargo.toml` (cargo add)
- Test: `crates/rf-core/tests/entry.rs`

**Interfaces:**
- Consumes: 없음
- Produces:
  - `Attributes` (bitflags, `u32`, `Clone, Copy, Debug, Default, PartialEq, Eq, Hash`): 아래 비트 값은 저장·IPC에 쓰일 수 있으므로 바꾸지 않는다. OS 플래그와의 대응은 rf-platform이 한다(예: Windows `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`/`OFFLINE` → `OFFLINE`, macOS `UF_HIDDEN`과 점 파일 → `HIDDEN`).
  - `EntryKind { File, Dir, Symlink }` (`Clone, Copy, Debug, PartialEq, Eq, Hash`, `non_exhaustive` 아님). Windows 정션도 `Symlink`이고 `REPARSE_POINT`가 함께 선다.
  - `VolumeId(pub u64)`: Windows 볼륨 일련번호, Unix `st_dev`. `FileId(pub u128)`: 볼륨 안에서 유일한 ID(NTFS 64비트, ReFS 128비트 `FILE_ID_128`, Unix `st_ino`). 둘 다 `Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord`.
  - `Entry` (`Clone, Debug, PartialEq, Eq`, `non_exhaustive`, 필드는 모두 `pub`): 목록의 한 항목. `name`은 부모 폴더 기준 이름 하나이고 전체 경로는 `dir.join(&entry.name)`이다. 다른 크레이트는 `Entry::new`로 만들고 필드를 채운다(구조체 리터럴 불가, 그래서 필드를 더해도 깨지지 않는다).
  - `pub fn new(name: impl Into<OsString>, kind: EntryKind) -> Entry`: 크기 0, 시각 `None`, 속성 비어 있음, ID `None`
  - `pub fn extension(&self) -> Option<&OsStr>`: NAV-07 확장자 열
  - `pub fn identity(&self) -> Option<(VolumeId, FileId)>`: META-03 재연결 키, 둘 다 있을 때만 `Some`
  - 재수출: `rf_core::{Attributes, Entry, EntryKind, FileId, VolumeId}`

- [ ] **Step 1: 의존성을 추가한다**

Run: `cargo add -p rf-core bitflags`
Expected: `Adding bitflags v2.` 로 시작하는 줄

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`crates/rf-core/tests/entry.rs`:

```rust
use std::ffi::OsString;

use rf_core::{Attributes, Entry, EntryKind, FileId, VolumeId};

#[test]
fn new_entry_has_empty_metadata() {
    let e = Entry::new("a.txt", EntryKind::File);
    assert_eq!(e.name, OsString::from("a.txt"));
    assert_eq!(e.kind, EntryKind::File);
    assert_eq!(e.size, 0);
    assert_eq!((e.modified, e.created, e.accessed), (None, None, None));
    assert_eq!(e.attributes, Attributes::empty());
    assert_eq!((e.volume_id, e.file_id), (None, None));
}

#[test]
fn extension_rules() {
    let ext = |n: &str, k| Entry::new(n, k).extension().map(|e| e.to_string_lossy().into_owned());
    assert_eq!(ext("a.tar.gz", EntryKind::File).as_deref(), Some("gz"));
    assert_eq!(ext(".bashrc", EntryKind::File), None);
    assert_eq!(ext("a.", EntryKind::File), None);
    assert_eq!(ext("noext", EntryKind::File), None);
    assert_eq!(ext("photos.2026", EntryKind::Dir), None);
    assert_eq!(ext("link.lnk", EntryKind::Symlink).as_deref(), Some("lnk"));
}

#[test]
fn identity_needs_both_ids() {
    let mut e = Entry::new("a", EntryKind::File);
    e.volume_id = Some(VolumeId(7));
    assert_eq!(e.identity(), None);
    e.file_id = Some(FileId(u128::MAX));
    assert_eq!(e.identity(), Some((VolumeId(7), FileId(u128::MAX))));
}

#[test]
fn attribute_bits_are_stable() {
    let all = [
        (Attributes::READONLY, 1 << 0),
        (Attributes::HIDDEN, 1 << 1),
        (Attributes::SYSTEM, 1 << 2),
        (Attributes::ARCHIVE, 1 << 3),
        (Attributes::COMPRESSED, 1 << 4),
        (Attributes::ENCRYPTED, 1 << 5),
        (Attributes::SPARSE, 1 << 6),
        (Attributes::OFFLINE, 1 << 7),
        (Attributes::REPARSE_POINT, 1 << 8),
    ];
    for (flag, bits) in all {
        assert_eq!(flag.bits(), bits);
    }
    let hs = Attributes::HIDDEN | Attributes::SYSTEM;
    assert!(hs.contains(Attributes::HIDDEN) && !hs.contains(Attributes::READONLY));
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test -p rf-core --test entry`
Expected: FAIL, `error[E0432]: unresolved imports `rf_core::Attributes`, `rf_core::Entry`, `rf_core::EntryKind`, `rf_core::FileId`, `rf_core::VolumeId``

- [ ] **Step 4: `entry.rs`를 구현하고 lib.rs에 `pub mod entry;`와 재수출을 추가한다**

```rust
bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
    pub struct Attributes: u32 {
        const READONLY = 1 << 0;
        const HIDDEN = 1 << 1;
        const SYSTEM = 1 << 2;
        const ARCHIVE = 1 << 3;
        const COMPRESSED = 1 << 4;
        const ENCRYPTED = 1 << 5;
        const SPARSE = 1 << 6;
        const OFFLINE = 1 << 7;
        const REPARSE_POINT = 1 << 8;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Entry {
    pub name: OsString,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub attributes: Attributes,
    pub volume_id: Option<VolumeId>,
    pub file_id: Option<FileId>,
}
```

`extension`: `Dir`이면 `None`, 그 밖에는 `Path::new(&self.name).extension()`에서 빈 문자열을 `None`으로 거른다.

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test -p rf-core --test entry`
Expected: `test result: ok. 4 passed`

- [ ] **Step 6: 표준 검사를 돌린다**

- [ ] **Step 7: 커밋한다**

```bash
git add Cargo.lock crates/rf-core
git commit -m "feat(rf-core): add Entry metadata model"
```

---

### Task 9: AppPaths와 포터블 모드

**Files:**
- Create: `crates/rf-core/src/os.rs`, `crates/rf-core/src/app_paths.rs`
- Modify: `crates/rf-core/src/lib.rs`, `crates/rf-core/Cargo.toml` (cargo add)
- Test: `crates/rf-core/tests/app_paths.rs`

**Interfaces:**
- Consumes: `identity::{BUNDLE_ID, DATA_DIR_NAME, PORTABLE_DATA_DIR, PORTABLE_MARKER}` (Task 1), `Error::{MissingEnv, Path, io}`, `Result` (Task 7), `VPath::local` (Task 4)
- Produces:
  - `rf_core::os::Os { Windows, MacOs, Other }` (`Clone, Copy, Debug, PartialEq, Eq, Hash`), `pub const CURRENT: Os` (`cfg(target_os)`로 고름, 그 밖의 타깃은 `Other`). 호스트와 무관한 순수 로직이 어느 OS 규칙을 따를지 인자로 받을 때 쓴다.
  - `rf_core::app_paths::InstallMode { Installed, Portable }` (`Clone, Copy, Debug, PartialEq, Eq, Hash`)
  - `rf_core::app_paths::BaseDirs` (`Clone, Debug, Default, PartialEq, Eq`, 필드 `pub`): `home`, `app_data`(`%APPDATA%`), `local_app_data`(`%LOCALAPPDATA%`), `temp`. `pub fn from_env() -> BaseDirs`: 환경 변수 `HOME`, `APPDATA`, `LOCALAPPDATA`(빈 값은 `None`), `temp = std::env::temp_dir()`
  - `rf_core::app_paths::AppPaths` (`Clone, Debug, PartialEq, Eq`, 필드 비공개): `resolve`, `detect`, 접근자, `owned_paths`, `ensure_dirs` (요약 절의 시그니처). 후속 계획 규칙: 설정 TOML은 `config_dir()`, 원장·작업 저널·메타 DB·색인 스냅샷은 `data_dir()`, 썸네일 등은 `cache_dir()`, 로그는 `log_dir()`, 임시 파일은 `temp_dir()` 안에 `identity::TEMP_PREFIX`로 시작하는 이름으로 만든다. 그 밖의 위치에는 쓰지 않는다(INST-03). 테스트는 `AppPaths::resolve`에 tempdir 기반 `BaseDirs`와 가짜 실행 파일 경로를 넘겨 실제 기기와 무관하게 만든다.
  - 재수출: `rf_core::{AppPaths, BaseDirs, InstallMode, Os}`

배치표(`L` = `local_app_data/RacoonFinder`, `S` = `home/Library/Application Support/<BUNDLE_ID>`, `R` = 포터블 루트 `<exe 폴더>/data`, `X` = `home/.local/share/RacoonFinder`):

| 항목 | Windows 설치형 | Windows 포터블 | macOS | Other(개발용) |
|---|---|---|---|---|
| `config_dir` | `app_data/RacoonFinder` | `R/config` | `S` | `X/config` |
| `data_dir` | `L/state` | `R/state` | `S/state` | `X/state` |
| `cache_dir` | `L/cache` | `R/cache` | `home/Library/Caches/<BUNDLE_ID>` | `X/cache` |
| `log_dir` | `L/logs` | `R/logs` | `home/Library/Logs/<BUNDLE_ID>` | `X/logs` |
| `temp_dir` | `base.temp` | `R/tmp` | `base.temp` | `base.temp` |
| `preferences_file` | `None` | `None` | `home/Library/Preferences/<BUNDLE_ID>.plist` | `None` |
| `owned_paths` | `[app_data/RacoonFinder, L]` | `[R]` | `[S, Caches/<ID>, Logs/<ID>, Preferences/<ID>.plist]` | `[X]` |

Windows 설치형 데이터 폴더(`state`)를 Roaming이 아닌 Local에 두는 이유: 원장·파일 ID·색인은 기기마다 다르므로 로밍 프로필로 다른 PC에 넘어가면 안 된다. 포터블 마커는 Windows와 Other에서만 보고, macOS에서는 무시한다(앱 번들 안이며 PRD는 포터블을 Windows 전용으로 정함).

- [ ] **Step 1: 개발 의존성을 추가한다**

Run: `cargo add -p rf-core --dev tempfile`
Expected: `Adding tempfile v3.` 로 시작하는 줄

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`crates/rf-core/tests/app_paths.rs`:

```rust
use std::path::{Path, PathBuf};

use rf_core::identity::{BUNDLE_ID, DATA_DIR_NAME};
use rf_core::{AppPaths, BaseDirs, Error, InstallMode, Os, PathError};

struct Fake {
    _dir: tempfile::TempDir,
    root: PathBuf,
    exe: PathBuf,
    base: BaseDirs,
}

fn fake() -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::create_dir(root.join("bin")).unwrap();
    let base = BaseDirs {
        home: Some(root.join("home")),
        app_data: Some(root.join("Roaming")),
        local_app_data: Some(root.join("Local")),
        temp: root.join("Temp"),
    };
    Fake { exe: root.join("bin").join("app.exe"), _dir: dir, root, base }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn windows_installed_layout() {
    let f = fake();
    let p = AppPaths::resolve(Os::Windows, &f.exe, &f.base).unwrap();
    let roaming = f.root.join("Roaming").join(DATA_DIR_NAME);
    let local = f.root.join("Local").join(DATA_DIR_NAME);
    assert_eq!(p.mode(), InstallMode::Installed);
    assert_eq!(p.config_dir(), roaming);
    assert_eq!(p.data_dir(), local.join("state"));
    assert_eq!(p.cache_dir(), local.join("cache"));
    assert_eq!(p.log_dir(), local.join("logs"));
    assert_eq!(p.temp_dir(), f.root.join("Temp"));
    assert_eq!(p.preferences_file(), None);
    assert_eq!(p.owned_paths(), [roaming, local]);
}

#[test]
fn macos_layout() {
    let f = fake();
    let p = AppPaths::resolve(Os::MacOs, &f.exe, &f.base).unwrap();
    let lib = f.root.join("home").join("Library");
    let support = lib.join("Application Support").join(BUNDLE_ID);
    let caches = lib.join("Caches").join(BUNDLE_ID);
    let logs = lib.join("Logs").join(BUNDLE_ID);
    let plist = lib.join("Preferences").join(format!("{BUNDLE_ID}.plist"));
    assert_eq!(p.mode(), InstallMode::Installed);
    assert_eq!(p.config_dir(), support);
    assert_eq!(p.data_dir(), support.join("state"));
    assert_eq!(p.cache_dir(), caches);
    assert_eq!(p.log_dir(), logs);
    assert_eq!(p.preferences_file(), Some(plist.as_path()));
    assert_eq!(p.owned_paths(), [support, caches, logs, plist]);
}

#[test]
fn portable_marker_puts_everything_under_data() {
    let f = fake();
    std::fs::write(f.root.join("bin").join("portable"), b"").unwrap();
    let only_temp = BaseDirs { temp: f.base.temp.clone(), ..BaseDirs::default() };
    let p = AppPaths::resolve(Os::Windows, &f.exe, &only_temp).unwrap();
    let data = f.root.join("bin").join("data");
    assert_eq!(p.mode(), InstallMode::Portable);
    assert_eq!(p.config_dir(), data.join("config"));
    assert_eq!(p.data_dir(), data.join("state"));
    assert_eq!(p.cache_dir(), data.join("cache"));
    assert_eq!(p.log_dir(), data.join("logs"));
    assert_eq!(p.temp_dir(), data.join("tmp"));
    assert_eq!(p.owned_paths(), [data]);
}

#[test]
fn portable_needs_a_file_and_is_ignored_on_macos() {
    let f = fake();
    std::fs::create_dir(f.root.join("bin").join("portable")).unwrap();
    assert_eq!(AppPaths::resolve(Os::Windows, &f.exe, &f.base).unwrap().mode(), InstallMode::Installed);
    let g = fake();
    std::fs::write(g.root.join("bin").join("portable"), b"").unwrap();
    assert_eq!(AppPaths::resolve(Os::MacOs, &g.exe, &g.base).unwrap().mode(), InstallMode::Installed);
}

#[test]
fn ensure_dirs_creates_only_the_layout() {
    let f = fake();
    std::fs::write(f.root.join("bin").join("portable"), b"").unwrap();
    let p = AppPaths::resolve(Os::Windows, &f.exe, &f.base).unwrap();
    p.ensure_dirs().unwrap();
    for d in [p.config_dir(), p.data_dir(), p.cache_dir(), p.log_dir(), p.temp_dir()] {
        assert!(d.is_dir(), "{}", d.display());
    }
    assert_eq!(names(&f.root.join("bin")), ["data", "portable"]);
    assert_eq!(names(&f.root), ["bin"]);
}

#[test]
fn missing_or_relative_inputs_are_errors() {
    let f = fake();
    let no_appdata = BaseDirs { app_data: None, ..f.base.clone() };
    assert!(matches!(AppPaths::resolve(Os::Windows, &f.exe, &no_appdata), Err(Error::MissingEnv("APPDATA"))));
    let no_local = BaseDirs { local_app_data: None, ..f.base.clone() };
    assert!(matches!(AppPaths::resolve(Os::Windows, &f.exe, &no_local), Err(Error::MissingEnv("LOCALAPPDATA"))));
    let no_home = BaseDirs { home: None, ..f.base.clone() };
    assert!(matches!(AppPaths::resolve(Os::MacOs, &f.exe, &no_home), Err(Error::MissingEnv("HOME"))));
    let rel = BaseDirs { app_data: Some(PathBuf::from("Roaming")), ..f.base.clone() };
    assert!(matches!(AppPaths::resolve(Os::Windows, &f.exe, &rel), Err(Error::Path(PathError::NotAbsolute(_)))));
    assert!(matches!(
        AppPaths::resolve(Os::Windows, Path::new("app.exe"), &f.base),
        Err(Error::Path(PathError::NotAbsolute(_)))
    ));
}

#[test]
fn other_os_uses_home_local_share() {
    let f = fake();
    let p = AppPaths::resolve(Os::Other, &f.exe, &f.base).unwrap();
    let root = f.root.join("home").join(".local").join("share").join(DATA_DIR_NAME);
    assert_eq!(p.config_dir(), root.join("config"));
    assert_eq!(p.owned_paths(), [root]);
}

#[test]
fn from_env_reads_temp_dir() {
    assert_eq!(BaseDirs::from_env().temp, std::env::temp_dir());
}
```

- [ ] **Step 3: 실패를 확인한다**

Run: `cargo test -p rf-core --test app_paths`
Expected: FAIL, `error[E0432]: unresolved imports `rf_core::AppPaths`, `rf_core::BaseDirs`, `rf_core::InstallMode`, `rf_core::Os``

- [ ] **Step 4: `os.rs`와 `app_paths.rs`를 구현하고 lib.rs에 `pub mod os; pub mod app_paths;`와 재수출을 추가한다**

`resolve(os, exe, base)` 순서(테스트가 이 순서에 의존한다):
1. `exe`가 절대 경로가 아니면 `Error::Path(PathError::NotAbsolute(lossy))`.
2. `os != Os::MacOs`이고 `exe.parent()/PORTABLE_MARKER`가 `is_file()`이면 포터블 배치를 돌려준다. 이때 `base`는 보지 않는다.
3. `base.temp`가 절대 경로가 아니면 `NotAbsolute`.
4. OS별: Windows는 `app_data`(없으면 `MissingEnv("APPDATA")`)를 먼저, 그다음 `local_app_data`(`MissingEnv("LOCALAPPDATA")`)를 확인한다. macOS·Other는 `home`(`MissingEnv("HOME")`). 쓰는 기준 폴더가 상대 경로면 `NotAbsolute`. 그 뒤 배치표대로 조립한다. 하위 폴더 이름 `config`, `state`, `cache`, `logs`, `tmp`는 `app_paths.rs`의 비공개 상수로 둔다.

`detect()`: `std::env::current_exe()`(실패는 `Error::Unsupported(format!("current_exe: {e}"))`) → `resolve(Os::CURRENT, &exe, &BaseDirs::from_env())`.
`ensure_dirs()`: `config_dir`, `data_dir`, `cache_dir`, `log_dir`, `temp_dir` 순서로 `std::fs::create_dir_all`. 실패하면 `Error::io("create_dir_all", VPath::local(dir)?, e)`.

- [ ] **Step 5: 통과를 확인한다**

Run: `cargo test -p rf-core --test app_paths`
Expected: `test result: ok. 8 passed`

- [ ] **Step 6: 표준 검사와 교차 검사를 돌린다** (`Os::CURRENT`의 `cfg`가 들어온다)

- [ ] **Step 7: 커밋한다**

```bash
git add Cargo.lock crates/rf-core
git commit -m "feat(rf-core): add AppPaths with portable mode"
```

---

### Task 10: 최종 교차 검증과 CI 확인

**Files:**
- 수정 없음(실패가 나오면 고친 파일만 커밋)

**Interfaces:**
- Consumes: Task 1~9 전체
- Produces: 네 OS CI와 cargo-deny가 통과한 `rf-core` 0.1.0. 후속 M1 계획(rf-platform·rf-vfs, rf-jobs, rf-ledger)의 출발점.

- [ ] **Step 1: 잠금 파일 기준으로 전체 테스트를 돌린다**

Run: `cargo test --workspace --locked`
Expected (Linux): `unittests src/lib.rs` 10 passed, `tests/app_paths.rs` 8, `tests/entry.rs` 4, `tests/error.rs` 5, `tests/identity.rs` 2, `tests/vpath_local.rs` 10, `tests/vpath_nav.rs` 4, `tests/vpath_scheme.rs` 7, 문서 테스트 0. 모든 줄이 `test result: ok.`

- [ ] **Step 2: 표준 검사와 두 타깃 교차 검사를 돌린다**

Run: `cargo fmt --all --check` 그리고 `cargo clippy --workspace --all-targets --locked -- -D warnings` 그리고 Global Constraints의 교차 검사 두 명령
Expected: fmt 출력 없음, clippy 세 번 모두 `Finished`로 끝남

- [ ] **Step 3: cargo-deny 전체 검사를 돌린다**

Run: `cargo deny check`
Expected: `advisories ok, bans ok, licenses ok, sources ok`

- [ ] **Step 4: 브랜치를 올리고 CI를 지켜본다** (원격에 올리는 일이므로 사용자의 확인을 받은 뒤에만 한다)

Run: `git push -u origin HEAD` 그리고 `gh run watch --exit-status $(gh run list --branch "$(git branch --show-current)" --limit 1 --json databaseId --jq '.[0].databaseId')`
Expected: 종료 코드 0. `gh run view --json jobs --jq '.jobs[] | "\(.name) \(.conclusion)"'` 출력이 `test (ubuntu-latest) success`, `test (windows-latest) success`, `test (windows-11-arm) success`, `test (macos-15) success`, `cargo-deny success` 다섯 줄. Windows 작업의 `tests/vpath_local.rs`는 `12 passed`, `tests/vpath_nav.rs`는 `7 passed`여야 한다(`cfg(windows)` 테스트가 실제로 돈 증거). `windows-11-arm` 작업이 러너를 찾지 못하고 대기하면(비공개 저장소) 매트릭스를 고치지 말고 사용자에게 알린다.

- [ ] **Step 5: 고친 것이 있으면 커밋한다**

```bash
git add -A crates Cargo.toml Cargo.lock deny.toml .github
git commit -m "fix(rf-core): address cross-platform CI findings"
```
