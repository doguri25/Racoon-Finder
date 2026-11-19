# M0 기술 검증(스파이크) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** PRD 10장 "M0 기술 검증"의 Slint 게이트 6개와 "함께 검증할 것" 전 항목, 그리고 PRD 곳곳의 M0 확인 항목(WINX-07, WINX-02 `/select` 행, MACX-01·R5, IDX-04 `searchfs`, PERF-13, A11Y-02, R9, Q7, 8.5 cargo-deny)을 버리는 스파이크로 측정해 `docs/spikes/m0-report.md`에 PASS/FAIL로 남기고, 결정 게이트에서 기술 결정을 확정해 PRD에 반영한다.

**Architecture:** 스파이크마다 `spikes/<topic>/`에 독립 cargo 프로젝트를 만든다. 각 프로젝트는 빈 `[workspace]` 표를 가진 자체 워크스페이스라서 제품 워크스페이스의 회원이 아니며, 제품 크레이트는 스파이크를 절대 가져다 쓰지 않는다. 각 harness는 무엇을 어떤 API로 재는지 정해져 있고, 결과를 `RESULT` 한 줄로 출력한다. 실행자는 그 값을 보고서의 고정 표에 옮긴다. 이식 가능한 검증(데이터셋 생성기, 압축 라이브러리, OpenDAL, 스냅샷 비교 로직, 라이선스)은 Linux 컨테이너에서, 나머지는 Windows 11·macOS 15 실기에서 돌린다. 마지막 과제가 결정 표를 채우고 PRD 8.1·8.4·12·13장(과 결정을 미뤄 둔 문장들)을 고친다.

**Tech Stack:** Rust stable(edition 2024), Slint 1.18(winit 백엔드, femtovg·skia·software 렌더러), windows-rs 0.62, objc2 0.6·objc2-app-kit 0.3·objc2-foundation 0.3·objc2-service-management 0.3, Apache OpenDAL 0.59, russh·russh-sftp, zip 9, sevenz-rust2 0.24, libarchive(`bsdtar`), mft 0.7·ntfs 0.4, fsevent-sys 5, xxhash-rust, sysinfo, Inno Setup, Hyper-V, tart, cargo-deny. 버전은 2026-10-22 crates.io 기준 참고값이고 실제 버전은 `cargo add`가 고른다.

**Spec:** docs/PRD.md

## Global Constraints

- 언어와 도구: Rust stable(작성 시점 rustc 1.97.0), `edition = "2024"`, `license = "GPL-3.0-or-later"`(PRD Q2 제안), `publish = false`.
- 스파이크 `Cargo.toml` 템플릿(모든 스파이크 공통, 패키지 이름은 각 과제의 `cargo new --name` 값):
  ```toml
  [package]
  name = "rfspike-<name>"
  version = "0.0.0"
  edition = "2024"
  license = "GPL-3.0-or-later"
  publish = false

  [workspace]
  ```
- 스파이크는 `spikes/<topic>/`에만 두고, 제품 워크스페이스(루트 `Cargo.toml`, `crates/`)는 M0에서 만들지 않는다. 제품 크레이트 이름(PRD 8.2: rf-core, rf-platform, rf-vfs, rf-jobs, rf-ledger, …)은 M1 계획부터 쓴다. 스파이크 코드는 버린다(PRD 10장 "이 단계에서 만든 코드는 버린다").
- 스파이크에는 `rust-version`을 쓰지 않는다. edition 2024의 resolver 3은 `rust-version`에 맞춰 의존성을 낮은 버전으로 고르므로, `rust-version = "1.85"`를 쓰면 `cargo add slint`가 1.17 이전(드래그 앤 드롭 없음) 버전을 고를 수 있다. 2026-10-22 crates.io 값: slint 1.18.1 → rust-version 1.92, opendal 0.59.5 → 1.91, zip 9.0.5 → 1.88, sevenz-rust2 0.24.1 → 1.93, sysinfo 0.39.6 → 1.95. 제품 워크스페이스의 rust-version("1.85" 제안)은 Task 21의 표를 근거로 Task 22에서 확정한다.
- 의존성은 `cargo add --manifest-path spikes/<topic>/Cargo.toml <dep>`로만 추가한다(스파이크는 워크스페이스 밖이라 `-p` 대신 `--manifest-path`). 버전 번호를 손으로 쓰지 않는다. 결과를 좌우하는 하한만 `slint@1.17`처럼 준다.
- `windows`·`objc2-*` 크레이트의 기능 목록은 필요한 최소 집합으로 적었다. 컴파일 오류가 빠진 기능(예: `Win32_System_Memory`, `NSGraphics`)을 가리키면 그 기능만 같은 `cargo add ... -F` 명령으로 더한다.
- 스파이크는 제품 표시 이름과 데이터 폴더 이름을 쓰지 않는다(PRD Q1 미정, 제품에서는 rf-core 상수에만 둔다). 식별자는 `rfspike` 접두사, 번들 ID는 `io.github.rfspike.<topic>`, 레지스트리 키는 `HKCU\Software\RFSpike`, 데이터 폴더는 `RFSpike`, Named Pipe는 `\\.\pipe\rfspike-*`.
- OS별 코드는 `#[cfg(target_os = "windows")]` / `#[cfg(target_os = "macos")]` 뒤에 둔다. 한 OS 전용 스파이크의 `main`은 다른 OS에서 `<package>: Windows 전용`(또는 `macOS 전용`)을 출력하고 종료 코드 0으로 끝난다. Linux에서 테스트하는 순수 로직은 OS 코드와 다른 모듈에 둔다.
- 지원 플랫폼(PRD 4.1): Windows 11 x64·ARM64, macOS 15 Sequoia Apple Silicon(arm64)만.
- 측정 기준 환경(PRD 7.1 그대로): "Windows: 8코어 x64 CPU, 16GB RAM, NVMe SSD 2개. macOS: Apple M1, 16GB RAM. 릴리스 빌드에서 측정한다."
- 결과 줄: 판정에 쓰는 값은 `RESULT <표 ID> key=value ... verdict=<PASS|FAIL|INFO|INVALID>` 한 줄로 출력한다. 디버그 빌드(`cfg!(debug_assertions)`)에서는 항상 `verdict=INVALID reason=debug-build`. 판정에는 `--release` 빌드 결과만 쓴다.
- 반복: 시간 측정은 5회(콜드 스타트는 10회) 반복의 중앙값으로 판정하고, 최솟값·최댓값은 메모에 적는다.
- 콜드 캐시: Windows는 `C:\tools\RAMMap64.exe -accepteula -Et`(Sysinternals RAMMap, 대기 목록 비우기), macOS는 `sudo -n purge`(먼저 `sudo -v`). 비우기 명령이 0이 아닌 코드로 끝나면 그 측정은 `verdict=INVALID reason=flush-failed`.
- 전원: 측정 중 AC 연결. Windows 전원 모드 "최고 성능", macOS 저전력 모드 끔.
- 보고서 표 열은 고정이다: `| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |`. 판정 값은 `PASS`, `FAIL`, `참고`, `미측정` 넷뿐이다. 환경 열에는 보고서 1장의 환경 ID(E-W1 등)를 쓴다.
- 데이터 위치: 대용량 데이터는 환경 변수 `RFSPIKE_DATA`(원본 NVMe)와 `RFSPIKE_DST`(다른 NVMe) 아래에만 만든다. 100만 개 데이터셋은 Windows는 VHDX(`R:\`), macOS는 희소 APFS 이미지(`/Volumes/RFSPIKE1M`) 안에만 만든다. 저장소에는 데이터·비밀값·키를 커밋하지 않는다.
- OS 설정 변경(레지스트리, `defaults`, 서비스, 로그인 항목)은 `revert`가 있는 스파이크 도구로만 하고, 바꾸기 전에 백업을 남긴다.
- 네트워크는 SFTP 테스트 서버, OneDrive(Microsoft Graph), 크레이트·도구 다운로드에만 쓴다(PRD PRIV-02).
- 커밋: conventional commits, 영어. harness는 `chore(spike): ...`, 보고서는 `docs(spike): ...`, PRD는 `docs(prd): ...`. 브랜치는 현재 브랜치를 그대로 쓴다.

## Review Focus

PRD가 암시하지만 측정 절차가 놓치기 쉬운 것, 사람에게 가장 먼저 문제를 일으킬 순서다. 각 줄의 검사는 표시한 과제에 들어 있다.

1. **경로 전달 손상(RF1).** 드라이브 루트 `C:\`가 `"%1"`로 넘어오면 MSVC 인자 규칙 때문에 `C:"`가 된다. 공백·한글·이모지·`%`·`&`·260자 초과 경로도 런처와 Named Pipe를 거쳐 바이트 그대로 도착해야 한다. → Task 9 Step 2(단위 테스트)·Step 8(C1-7).
2. **무버퍼 I/O의 끝자락(RF2).** 섹터 크기의 배수가 아닌 파일(10,000,000,007바이트)을 무버퍼로 복사해도 크기와 xxh3가 원본과 같아야 한다. 빠르지만 틀린 복사는 PERF-08 PASS가 아니다. → Task 13 Step 2·Task 14 Step 3·Step 7(C5-5).
3. **거짓 "차이 0"(RF3).** 스냅샷 비교가 모든 것을 허용 목록으로 지우면 INST-07이 의미가 없다. 원장을 우회해 남긴 값 하나(`--leak`)가 정확히 한 건의 차이로 보고돼야 한다. → Task 19 Step 2·Step 9(C7-2), Task 20 Step 6(C7-4).
4. **색인 항목 집합 불일치(RF4).** MFT의 미사용 레코드, DOS 8.3 이름, 하드 링크의 두 번째 이름, 볼륨 루트의 OS 항목, 펌링크 중복을 잘못 세면 개수가 틀린다. 개수만이 아니라 상대 경로 집합의 해시가 데이터셋 매니페스트와 같아야 한다. → Task 1 Step 1(정의)·Task 15 Step 4(C9-4)·Task 16 Step 3(C10-6, C10-7).
5. **가짜 콜드 측정(RF5).** 디버그 빌드나 캐시 비우기 실패 상태의 숫자가 콜드 결과로 기록되면 게이트 5와 PERF-07이 틀린 결론을 낸다. → Task 4 Step 2·Step 6, Task 13 Step 2·Task 14 Step 2.

---

## 실행 환경 지도

| 환경 | 과제 |
|---|---|
| Linux 컨테이너 | 1, 13, 17, 18, 19의 Step 1~3, 21 |
| Windows 11 실기 | 2, 4·5·6·8의 Windows 단계, 7, 9, 10, 11, 14의 Windows 단계, 15, 19 |
| macOS 15 실기 | 3, 4·5·6·8의 macOS 단계, 12, 14의 macOS 단계, 16, 20 |
| 어디서나 | 22(모든 과제가 끝난 뒤) |

두 OS가 필요한 과제(4, 5, 6, 8, 14)는 OS 세션마다 한 번씩 실행하고, 세션마다 자기 단계의 커밋으로 끝낸다.

## File Structure

```
spikes/
├─ README.md                      스파이크 규칙(버리는 코드, 독립 워크스페이스, RESULT 형식)
├─ .gitignore                     */target/, **/.secrets/, **/.keys/, *.vhdx, *.sparseimage
├─ deny.toml                      cargo-deny 라이선스 허용 목록 (Task 21)
├─ dataset-gen/                   rfspike-data: 데이터셋 생성·검증·환경 출력 (Task 1)
│  ├─ src/{lib.rs, plan.rs, generate.rs, verify.rs, env.rs, main.rs}
│  ├─ tests/generate.rs
│  └─ scripts/{win-make-vhdx.ps1, win-remove-vhdx.ps1, mac-make-image.sh, mac-remove-image.sh}
├─ slint-list/                    게이트 1·5: rfspike-list, rfspike-coldstart (Task 4)
│  ├─ build.rs, ui/app.slint, src/{lib.rs, stats.rs, main.rs, bin/coldstart.rs}
├─ slint-ime/                     게이트 2: rfspike-slint-ime (Task 5)
│  ├─ build.rs, ui/app.slint, src/main.rs
├─ slint-dnd/                     게이트 3: rfspike-slint-dnd (Task 6)
│  ├─ build.rs, ui/app.slint, src/{main.rs, win_dnd.rs, mac_dnd.rs}, assets/drop-test.html
├─ slint-shellmenu/               게이트 4: rfspike-slint-shellmenu (Task 7)
│  ├─ build.rs, ui/app.slint, src/{main.rs, shell_menu.rs}
├─ slint-a11y/                    게이트 6: rfspike-slint-a11y (Task 8)
│  ├─ build.rs, ui/app.slint, src/main.rs
├─ winx-launcher/                 WINX-03·02·07: rfspike-open/main/probe/register (Task 9~11)
│  ├─ src/{lib.rs, cmdline.rs, pipe.rs, ledger.rs, catch.rs, wine.rs}
│  └─ src/bin/{rfspike-open.rs, rfspike-main.rs, rfspike-probe.rs, rfspike-register.rs}
├─ macx-fileviewer/               MACX-01·R5: RFSpikeViewer.app (Task 12)
│  ├─ src/main.rs, Info.plist, scripts/{bundle.sh, set-viewer.sh}
├─ copy-engine/                   PERF-07·08: rfspike-copy (Task 13·14)
│  ├─ src/{lib.rs, verdict.rs, aligned.rs, small.rs, large.rs, seqwrite.rs, baseline.rs, main.rs}
├─ index-ntfs/                    PERF-13(NTFS)·IDX-02·03: rfspike-ntfs (Task 15)
│  ├─ src/{main.rs, paths.rs, mft.rs, enum_usn.rs, walk.rs, usn.rs}
├─ index-mac/                     PERF-13(macOS)·IDX-04: rfspike-macidx (Task 16)
│  ├─ src/{main.rs, paths.rs, bulk.rs, searchfs.rs, fsevents.rs, churn.rs}
├─ opendal-remote/                함께 검증·R9: rfspike-remote (Task 17)
│  ├─ src/{main.rs, caps.rs, sftp_opendal.rs, sftp_russh.rs, onedrive.rs}
│  ├─ scripts/sftp-server.sh
│  ├─ wincheck-openssh/{Cargo.toml, src/lib.rs}
│  └─ wincheck-russh/{Cargo.toml, src/lib.rs}
├─ archive-write/                 ZIP(AES)·7z 쓰기 비교: rfspike-arc (Task 18)
│  ├─ src/{main.rs, zipw.rs, sevenzw.rs, split.rs, inspect.rs}, scripts/bsdtar-probe.sh
└─ ledger-vm/                     INST-02·07, Q7: rfspike-ledger, rfspike-snap (Task 19·20)
   ├─ src/{lib.rs, snapshot.rs, diff.rs, win_ops.rs, mac_ops.rs}, src/bin/{rfspike-ledger.rs, rfspike-snap.rs}
   ├─ tests/diff.rs, allowlist-windows.txt, allowlist-macos.txt
   ├─ installer/rfspike.iss
   └─ scripts/{win-vm-pipeline.ps1, win-guest-run.ps1, mac-bundle.sh, mac-vm-pipeline.sh, mac-guest-run.sh}
docs/spikes/m0-report.md          보고서: 측정 규칙, 환경, 데이터셋, 게이트·점검 표, 결정 표
docs/PRD.md                       Task 22에서만 수정
```

---

### Task 1: 보고서 골격과 데이터셋 생성기

**Files:**
- Create: `spikes/README.md`, `spikes/.gitignore`
- Create: `spikes/dataset-gen/Cargo.toml`, `spikes/dataset-gen/src/{lib.rs, plan.rs, generate.rs, verify.rs, env.rs, main.rs}`
- Create: `spikes/dataset-gen/tests/generate.rs`
- Create: `spikes/dataset-gen/scripts/{win-make-vhdx.ps1, win-remove-vhdx.ps1, mac-make-image.sh, mac-remove-image.sh}`
- Create: `docs/spikes/m0-report.md`

**Interfaces:**
- Consumes: 없음.
- Produces:
  - CLI `rfspike-data` (`fixtures`를 뺀 생성 명령은 데이터셋 루트에 `rfspike-manifest.json`을 쓴다):
    - `big-folder --out <dir> [--count 100000]` → `<dir>/big-folder/`
    - `small-files --out <dir> [--count 10000]` → `<dir>/small-files/`
    - `large-file --out <dir> [--size 10000000007] [--name large.bin]` → `<dir>/large-file/<name>`
    - `thousand --out <dir>` → `<dir>/thousand/` (파일 1,000개, 각 100바이트, `t0000.txt`~`t0999.txt`)
    - `million --out <volume-root> [--tops 100] [--threads 8]` → 볼륨 루트에 직접
    - `fixtures --out <dir>` → `<dir>/fixtures/{dnd,menu,ime,reveal,launcher}`
    - `verify --root <dataset-root>` → `VERIFY ok kind=<k> entries=<n> paths_xxh3=<h>`(종료 0) 또는 `VERIFY mismatch ...`(종료 1)
    - `env` → `ENV os="…" kernel="…" cpu="…" cores=<n> threads=<n> ram_gib=<n> disks="<mount> <kind> <fs> <GiB>GiB; …"`
    - `clean --root <dataset-root>` → 루트에 매니페스트가 있을 때만 지우고 `CLEAN ok`
  - 매니페스트 JSON(필드 고정): `{"kind":"million","entries":1011101,"files":1000000,"dirs":10101,"hardlinks":1000,"bytes":0,"paths_xxh3":"<16 hex>","content_xxh3":null,"generator":"rfspike-data 0.0.0","params":{"tops":100}}`
  - `paths_xxh3` 정의: 데이터셋 루트 기준 상대 경로(구분자 `/`, NFC, 폴더 포함, 하드 링크는 이름마다 한 항목)를 바이트 순으로 정렬하고, 각 경로 뒤에 `\n`을 붙여 이은 바이트열의 xxh3_64를 소문자 16자리 16진수로 쓴 값. 제외: 루트 바로 아래의 `rfspike-manifest.json`, `System Volume Information`, `$RECYCLE.BIN`, `$`로 시작하는 이름, `.fseventsd`, `.Spotlight-V100`, `.Trashes`, `.TemporaryItems`와 그 하위, 그리고 모든 깊이의 `.DS_Store`. Task 15·16은 이 정의를 그대로 복사해 쓴다.
  - 보고서 표 ID(G1~G6, C1~C13, D)와 행 ID(예: `G1-1`).

- [ ] **Step 1: 스파이크 규칙 파일과 생성기 프로젝트를 만든다**

`spikes/README.md`에 세 문단을 쓴다: (1) 스파이크는 버리는 코드이고 제품 크레이트가 가져다 쓰지 않는다, (2) 각 디렉터리는 빈 `[workspace]`를 가진 독립 프로젝트다, (3) 판정 값은 `RESULT` 줄로만 낸다(전역 제약 인용). `spikes/.gitignore`에 `*/target/`, `**/target/`, `**/.secrets/`, `**/.keys/`, `*.vhdx`, `*.sparseimage`를 쓴다.

```bash
cargo new --vcs none --lib --name rfspike-data spikes/dataset-gen
# Now edit Cargo.toml to match the spike template in Global Constraints (name = "rfspike-data").
cargo add --manifest-path spikes/dataset-gen/Cargo.toml clap -F derive
cargo add --manifest-path spikes/dataset-gen/Cargo.toml xxhash-rust -F xxh3
cargo add --manifest-path spikes/dataset-gen/Cargo.toml serde -F derive
cargo add --manifest-path spikes/dataset-gen/Cargo.toml serde_json sysinfo
cargo add --manifest-path spikes/dataset-gen/Cargo.toml --dev tempfile
```

데이터셋 규칙(`plan.rs`가 고정한다):
- big-folder: 항목 `count`개 중 폴더 `count/20`개(`폴더_{i:05}`), 나머지 파일. 파일 이름은 `i % 4`로 `IMG_{i:06}.jpg` / `문서_{i}.txt` / `report-{i}.pdf` / `data{i}.csv`, 크기 `(i * 37) % 1024`바이트.
- small-files: `d000`~`d099` 폴더에 100개씩 `s{i:05}.bin`. 크기는 쌍 규칙: `j < count/2`에 대해 `d_j = xorshift64*(j+1) % 15_361`, `size[j] = 16_384 + d_j`, `size[j + count/2] = 16_384 - d_j`. 내용은 파일 번호를 시드로 한 xorshift64* 난수(압축 불가).
- large-file: 크기 기본 10,000,000,007바이트(10GB, 4096의 배수가 아니도록 7바이트를 더함). 내용은 시드 1의 xorshift64* 스트림을 4MiB 단위로 쓰고, 쓰면서 xxh3_64를 계산해 `content_xxh3`에 넣는다.
- million: `m{a:02}/n{b:02}/` (a < tops, b < 100) 아래 파일 100개씩, 전역 번호 `n = a*10_000 + b*100 + c`. 이름은 `n % 1000 == 0` → `🦝_{n:07}.txt`, 그 밖에 `n % 10 == 0` → `사진_{n:07}.jpg`, 나머지 `f_{n:07}.dat`. 모두 0바이트. `links/hl_{k:04}.dat`(k < 1000)는 `n = k*10 + 1` 파일의 하드 링크.
- fixtures: `dnd/보고서 한글.txt`, `dnd/photo 🦝.jpg`(임의 1KiB), `dnd/100% & co.txt`, `dnd/폴더 A/inner.txt`, `dnd/long/`(50자 폴더 이름 6단계 아래 `file.txt`, 상대 경로 300자 이상); `menu/a.txt`, `menu/b.txt`, `menu/c.zip`(빈 ZIP: `PK\x05\x06` 뒤 0바이트 18개), `menu/폴더/`; `ime/보고서.txt`, `ime/사진.jpg`, `ime/notes.md`, `ime/a.txt`, `ime/b.txt`; `reveal/보고서 한글.txt`; `launcher/` 아래 폴더 `rf spike 공백`, `한글 폴더`, `📁 emoji`, `100% & co`, `long/`(상대 경로 270자 이상의 폴더).

- [ ] **Step 2: 실패하는 테스트를 쓴다**

`spikes/dataset-gen/src/plan.rs` 하단 `#[cfg(test)]`:

```rust
#[test]
fn small_sizes_sum_exactly() {
    let s = small_file_sizes(10_000);
    assert_eq!(s.len(), 10_000);
    assert_eq!(s.iter().sum::<u64>(), 163_840_000);
    assert!(s.iter().all(|&x| (1_024..=31_744).contains(&x)));
}

#[test]
fn million_plan_counts() {
    let p = million_plan(100);
    assert_eq!((p.files, p.dirs, p.hardlinks, p.entries()), (1_000_000, 10_101, 1_000, 1_011_101));
}

#[test]
fn big_folder_counts() {
    let p = big_folder_plan(100_000);
    assert_eq!((p.files, p.dirs), (95_000, 5_000));
}

#[test]
fn million_names_follow_rules() {
    assert_eq!(million_file_name(1_000), "🦝_0001000.txt");
    assert_eq!(million_file_name(10), "사진_0000010.jpg");
    assert_eq!(million_file_name(7), "f_0000007.dat");
}

#[test]
fn paths_hash_is_order_independent() {
    let a = paths_xxh3(vec!["b".into(), "a/c".into(), "a".into()]);
    let b = paths_xxh3(vec!["a".into(), "a/c".into(), "b".into()]);
    assert_eq!(a, b);
    assert_eq!(a.len(), 16);
}
```

`spikes/dataset-gen/tests/generate.rs`:

```rust
#[test]
fn million_tops1_verifies() {
    let d = tempfile::tempdir().unwrap();
    let m = rfspike_data::generate::million(d.path(), 1, 4).unwrap();
    assert_eq!((m.files, m.dirs, m.hardlinks, m.entries), (10_000, 102, 1_000, 11_102));
    let v = rfspike_data::verify::verify(d.path()).unwrap();
    assert!(v.ok);
    assert_eq!(v.paths_xxh3, m.paths_xxh3);
}

#[test]
fn large_file_hash_roundtrip() {
    let d = tempfile::tempdir().unwrap();
    let m = rfspike_data::generate::large_file(d.path(), 1_000_007, "large.bin").unwrap();
    assert_eq!(m.bytes, 1_000_007);
    let v = rfspike_data::verify::verify(&d.path().join("large-file")).unwrap();
    assert!(v.ok);
}

#[test]
fn small_files_manifest_bytes() {
    let d = tempfile::tempdir().unwrap();
    let m = rfspike_data::generate::small_files(d.path(), 100).unwrap();
    assert_eq!((m.files, m.dirs, m.bytes), (100, 100, 1_638_400));
}
```

(`small_files(.., 100)`도 `d000`~`d099` 폴더 100개를 만들고 폴더마다 1개씩 둔다.)

- [ ] **Step 3: 테스트가 실패하는지 확인한다**

Run: `cargo test --release --manifest-path spikes/dataset-gen/Cargo.toml`
Expected: FAIL, `cannot find function `small_file_sizes` in this scope` 같은 컴파일 오류.

- [ ] **Step 4: 생성기를 구현한다**

`plan.rs`:
- `pub fn small_file_sizes(count: usize) -> Vec<u64>` (count가 홀수면 panic)
- `pub struct BigFolderPlan { pub files: u64, pub dirs: u64 }`, `pub fn big_folder_plan(count: u64) -> BigFolderPlan`, `pub fn big_folder_name(i: u64, is_dir: bool) -> String`
- `pub struct MillionPlan { pub tops: u32, pub files: u64, pub dirs: u64, pub hardlinks: u64 }`, `impl MillionPlan { pub fn entries(&self) -> u64 }`, `pub fn million_plan(tops: u32) -> MillionPlan` (dirs = tops + tops*100 + 1)
- `pub fn million_file_name(n: u64) -> String`
- `pub fn paths_xxh3(paths: Vec<String>) -> String`
- `pub fn is_excluded(rel_path: &str) -> bool` (Interfaces의 제외 규칙)

`generate.rs`: `pub struct Manifest { pub kind: String, pub entries: u64, pub files: u64, pub dirs: u64, pub hardlinks: u64, pub bytes: u64, pub paths_xxh3: String, pub content_xxh3: Option<String>, pub generator: String, pub params: serde_json::Value }`(serde)와 `pub fn big_folder(out: &Path, count: u64) -> io::Result<Manifest>`, `pub fn small_files(out: &Path, count: usize) -> io::Result<Manifest>`, `pub fn large_file(out: &Path, size: u64, name: &str) -> io::Result<Manifest>`, `pub fn thousand(out: &Path) -> io::Result<Manifest>`, `pub fn million(root: &Path, tops: u32, threads: usize) -> io::Result<Manifest>`(최상위 폴더 단위로 `std::thread::scope` 병렬 생성, 하드 링크는 `std::fs::hard_link`), `pub fn fixtures(out: &Path) -> io::Result<()>`.

`verify.rs`: `pub struct VerifyReport { pub ok: bool, pub kind: String, pub entries: u64, pub paths_xxh3: String }`, `pub fn verify(root: &Path) -> io::Result<VerifyReport>` — `std::fs::read_dir`로 재귀(심볼릭 링크 따라가지 않음), 제외 규칙 적용, 개수·`paths_xxh3`·(large-file이면) 크기와 `content_xxh3`를 매니페스트와 비교.

`env.rs`: `pub fn env_line() -> String` — sysinfo의 `System::long_os_version()`, `System::kernel_version()`, CPU 이름, 물리·논리 코어 수, 전체 메모리, `Disks::new_with_refreshed_list()`의 마운트·종류·파일 시스템·용량.

`main.rs`: clap 하위 명령으로 Interfaces의 CLI를 연결한다.

- [ ] **Step 5: 테스트가 통과하는지 확인한다**

Run: `cargo test --release --manifest-path spikes/dataset-gen/Cargo.toml`
Expected: `test result: ok. 5 passed`(lib)와 `test result: ok. 3 passed`(tests/generate.rs).

- [ ] **Step 6: 볼륨 스크립트를 쓴다**

`win-make-vhdx.ps1`(관리자 PowerShell, 매개변수 `-Path "$env:RFSPIKE_DATA\rfspike-1m.vhdx" -SizeGB 16 -Letter R`): diskpart 스크립트 파일을 만들어 `create vdisk file="<Path>" maximum=<SizeGB*1024> type=expandable` → `select vdisk file="<Path>"` → `attach vdisk` → `create partition primary` → `format fs=ntfs quick label=RFSPIKE1M` → `assign letter=R`를 실행한 뒤, `fsutil 8dot3name set R: 0`(8.3 이름 생성 켬, RF4 검사용)과 `Add-MpPreference -ExclusionPath R:\`(생성 속도)를 실행하고 `fsutil 8dot3name query R:`의 출력을 그대로 보여 준다.
`win-remove-vhdx.ps1`: `Remove-MpPreference -ExclusionPath R:\`, diskpart `select vdisk` → `detach vdisk`, VHDX 파일 삭제.
`mac-make-image.sh`: `hdiutil create -size 16g -type SPARSE -fs APFS -volname RFSPIKE1M "$RFSPIKE_DATA/rfspike-1m.sparseimage"` → `hdiutil attach "$RFSPIKE_DATA/rfspike-1m.sparseimage"` → `sudo mdutil -i off /Volumes/RFSPIKE1M`.
`mac-remove-image.sh`: `hdiutil detach /Volumes/RFSPIKE1M` → 이미지 파일 삭제.

- [ ] **Step 7: Linux에서 축소 데이터셋으로 동작을 확인한다**

```bash
export RFSPIKE_DATA=/tmp/rfspike-data && mkdir -p "$RFSPIKE_DATA"
B=spikes/dataset-gen/target/release/rfspike-data
cargo build --release --manifest-path spikes/dataset-gen/Cargo.toml
$B small-files --out "$RFSPIKE_DATA" --count 1000 && $B verify --root "$RFSPIKE_DATA/small-files"
$B million --out "$RFSPIKE_DATA/m1" --tops 1 && $B verify --root "$RFSPIKE_DATA/m1"
$B fixtures --out "$RFSPIKE_DATA" && $B env
```
Expected: `VERIFY ok kind=small-files entries=1100 ...`, `VERIFY ok kind=million entries=11102 ...`, 마지막 줄이 `ENV os="` 로 시작. 확인 후 `$B clean --root "$RFSPIKE_DATA/small-files"` → `CLEAN ok`.

- [ ] **Step 8: 보고서 골격을 쓴다**

`docs/spikes/m0-report.md`를 아래 내용 그대로 만든다(측정값·환경·메모 칸은 비우고 판정은 `미측정`).

````markdown
# M0 기술 검증 보고서

- 계획: docs/superpowers/plans/2026-10-22-m0-tech-spike.md
- 명세: docs/PRD.md 10장 "M0 기술 검증"
- 상태: 진행 중

## 0. 측정 규칙

- 기준 환경(PRD 7.1): Windows: 8코어 x64 CPU, 16GB RAM, NVMe SSD 2개. macOS: Apple M1, 16GB RAM. 릴리스 빌드에서 측정한다.
- 판정에는 `--release` 빌드의 `RESULT` 줄만 쓴다. `verdict=INVALID`는 판정에 쓰지 않고 다시 잰다.
- 시간은 5회(콜드 스타트 10회) 중앙값으로 판정하고 최솟값·최댓값을 메모에 적는다.
- 콜드 캐시: Windows `C:\tools\RAMMap64.exe -accepteula -Et`, macOS `sudo -n purge`.
- 판정 값: PASS / FAIL / 참고 / 미측정. `참고` 행은 게이트 판정에 들어가지 않는다.

## 1. 환경

| 환경 ID | `rfspike-data env` 출력 | 메모(디스크 모델·버스, 화면 주사율·배율, OS 빌드) |
|---|---|---|
| E-W1 | | Windows 11 x64 기준 장비 |
| E-W2 | | Windows 11 ARM64(있을 때만) |
| E-M1 | | macOS 15 Apple M1 기준 장비 |
| E-L1 | | Linux 컨테이너(이식 가능 항목) |

## 2. 데이터셋

| 이름 | 내용 | Windows 위치 | macOS 위치 | `VERIFY` 결과(Windows / macOS) |
|---|---|---|---|---|
| big-folder | 항목 100,000개(파일 95,000, 폴더 5,000) | `%RFSPIKE_DATA%\big-folder` | `$RFSPIKE_DATA/big-folder` | |
| small-files | 파일 10,000개, 평균 16KB(합계 163,840,000바이트), 폴더 100개 | `%RFSPIKE_DATA%\small-files` | `$RFSPIKE_DATA/small-files` | |
| large-file | 10,000,000,007바이트 파일 1개 | `%RFSPIKE_DATA%\large-file\large.bin` | `$RFSPIKE_DATA/large-file/large.bin` | |
| thousand | 파일 1,000개 | `%RFSPIKE_DATA%\thousand` | `$RFSPIKE_DATA/thousand` | |
| million | 파일 1,000,000 + 하드 링크 1,000 + 폴더 10,101 = 항목 1,011,101 | VHDX `R:\` | 희소 APFS 이미지 `/Volumes/RFSPIKE1M` | |
| fixtures | DnD·메뉴·IME·Reveal·런처 경로 시험용 | `%RFSPIKE_DATA%\fixtures` | `$RFSPIKE_DATA/fixtures` | 해당 없음(매니페스트 없음) |

## G1. 대용량 목록 (Slint 게이트 1)

판정 규칙: 렌더러 하나로 측정한 phase별 평균 fps ≥ 59.0이고, 25ms를 넘는 프레임 간격 비율 ≤ 1%이면 PASS. G1과 G5를 같은 렌더러로 모두 통과한 렌더러가 있으면 그 렌더러 값을 측정값에, 나머지 렌더러 값은 메모에 적는다.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| G1-1 Windows, 일정 속도 스크롤 | 10만 개 항목 목록을 60fps로 스크롤 | | | 미측정 | |
| G1-2 Windows, 임의 위치 점프 | 10만 개 항목 목록을 60fps로 스크롤 | | | 미측정 | |
| G1-3 macOS, 일정 속도 스크롤 | 10만 개 항목 목록을 60fps로 스크롤 | | | 미측정 | |
| G1-4 macOS, 임의 위치 점프 | 10만 개 항목 목록을 60fps로 스크롤 | | | 미측정 | |
| G1-5 120Hz 화면(참고) | 스크롤 60fps 유지(120Hz 기기에서는 120fps 목표) | | | 미측정 | |

## G2. 한글 IME (Slint 게이트 2)

판정 규칙: Task 5의 T1~T3 로그 조건을 모두 만족하면 PASS.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| G2-1 Windows(Microsoft 한국어 IME), 조합 표시 | 이름 바꾸기 입력란에서 조합 표시, 조합 중 Enter·Esc 정상 처리 | | | 미측정 | |
| G2-2 Windows, 조합 중 Enter | 이름 바꾸기 입력란에서 조합 표시, 조합 중 Enter·Esc 정상 처리 | | | 미측정 | |
| G2-3 Windows, 조합 중 Esc | 이름 바꾸기 입력란에서 조합 표시, 조합 중 Enter·Esc 정상 처리 | | | 미측정 | |
| G2-4 macOS(한국어 2벌식), 조합 표시 | 이름 바꾸기 입력란에서 조합 표시, 조합 중 Enter·Esc 정상 처리 | | | 미측정 | |
| G2-5 macOS, 조합 중 Enter | 이름 바꾸기 입력란에서 조합 표시, 조합 중 Enter·Esc 정상 처리 | | | 미측정 | |
| G2-6 macOS, 조합 중 Esc | 이름 바꾸기 입력란에서 조합 표시, 조합 중 Enter·Esc 정상 처리 | | | 미측정 | |
| G2-7 조합 중 Backspace·다른 곳 클릭(참고) | 모든 입력란에서 조합 중인 글자를 제대로 표시하고, 조합 중의 Enter와 Esc를 올바르게 처리한다 | | | 미측정 | |

## G3. 드래그 앤 드롭 (Slint 게이트 3)

판정 규칙: 픽스처 5개(한글·이모지·`%&`·폴더·긴 경로)가 이름과 SHA-256이 같게 도착하면 PASS. 측정값에 사용한 방식(winit / OLE / NSView)을 적는다.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| G3-1 탐색기 → 앱 | 탐색기·Finder와 양방향으로 파일을 주고받음(OS 계층 직접 구현 포함) | | | 미측정 | |
| G3-2 앱 → 탐색기 | 탐색기·Finder와 양방향으로 파일을 주고받음(OS 계층 직접 구현 포함) | | | 미측정 | |
| G3-3 Finder → 앱 | 탐색기·Finder와 양방향으로 파일을 주고받음(OS 계층 직접 구현 포함) | | | 미측정 | |
| G3-4 앱 → Finder | 탐색기·Finder와 양방향으로 파일을 주고받음(OS 계층 직접 구현 포함) | | | 미측정 | |
| G3-5 앱 → 브라우저 업로드(참고) | Racoon Finder에서 다른 앱(탐색기, Finder, 브라우저 업로드, 메일 첨부 등)으로 끌어낼 수 있다 | | | 미측정 | |
| G3-6 외부 드롭 동작 결정(참고) | 외부에서 들어온 드롭은 원본 앱이 허용하는 동작 중 설정값을 우선하고, 이동이 허용되지 않으면 복사한다 | | | 미측정 | |

## G4. 셸 메뉴 (Slint 게이트 4)

판정 규칙: G4-1~G4-4가 모두 로그 조건(Task 7)을 만족하면 PASS.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| G4-1 파일 1개 메뉴 표시 | Windows 네이티브 컨텍스트 메뉴를 앱 창에 띄움 | | | 미측정 | |
| G4-2 여러 항목(3개) 메뉴 | Windows 네이티브 컨텍스트 메뉴를 앱 창에 띄움 | | | 미측정 | |
| G4-3 하위 메뉴(보내기·연결 프로그램) 채워짐 | Windows 네이티브 컨텍스트 메뉴를 앱 창에 띄움 | | | 미측정 | |
| G4-4 명령 실행(속성·삭제·이름 바꾸기 동사) | Windows 네이티브 컨텍스트 메뉴를 앱 창에 띄움 | | | 미측정 | |
| G4-5 서드파티·Windows 11 새 메뉴 항목(참고) | 탐색기와 같은 메뉴(다른 앱이 등록한 항목, Windows 11 새 메뉴 항목 포함) | | | 미측정 | |
| G4-6 폴더 배경 메뉴(새로 만들기, 참고) | 탐색기와 같은 메뉴(다른 앱이 등록한 항목, Windows 11 새 메뉴 항목 포함) | | | 미측정 | |
| G4-7 앱 메뉴 위 + OS 메뉴 하위 메뉴(참고) | 앱 자체 메뉴가 위에 오고(맨 위는 '이동…', '복사…'), OS 메뉴는 하위 메뉴로 붙인다 | | | 미측정 | |

## G5. 시작 시간 (Slint 게이트 5)

판정 규칙: 콜드 10회 중앙값 ≤ 400ms이면 PASS(G1과 같은 렌더러). 재부팅 직후 1회 값은 메모에 적는다.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| G5-1 Windows 콜드 | 콜드 스타트 첫 화면 400ms 이하 | | | 미측정 | |
| G5-2 macOS 콜드 | 콜드 스타트 첫 화면 400ms 이하 | | | 미측정 | |
| G5-3 Windows 웜(참고) | 콜드 스타트 첫 화면 400ms 이하 | | | 미측정 | |
| G5-4 macOS 웜(참고) | 콜드 스타트 첫 화면 400ms 이하 | | | 미측정 | |

## G6. 스크린 리더 (Slint 게이트 6, 참고)

판정 규칙: 목록에서 ↓를 세 번 눌렀을 때 리더가 세 항목의 이름을 순서대로 읽으면 PASS. 미달이면 아래 "보완 계획"을 채운다.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| G6-1 내레이터, 목록 항목 | 목록 항목 이름을 읽음. 미달 시 보완 계획 수립 | | | 미측정 | |
| G6-2 NVDA, 목록 항목 | 목록 항목 이름을 읽음. 미달 시 보완 계획 수립 | | | 미측정 | |
| G6-3 VoiceOver, 목록 항목 | 목록 항목 이름을 읽음. 미달 시 보완 계획 수립 | | | 미측정 | |
| G6-4 Windows, 트리·버튼·대화상자 이름과 상태 | 스크린 리더(Windows 내레이터·NVDA, macOS VoiceOver)가 목록, 트리, 버튼, 대화상자의 이름과 상태를 읽는다 | | | 미측정 | |
| G6-5 macOS, 트리·버튼·대화상자 이름과 상태 | 스크린 리더(Windows 내레이터·NVDA, macOS VoiceOver)가 목록, 트리, 버튼, 대화상자의 이름과 상태를 읽는다 | | | 미측정 | |

보완 계획(미달 시): 문제(행 ID) / OS 계층 보완 방법 / 반영 마일스톤.

## C1. 탐색기 대체 런처 (WINX-03)

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C1-1 런처 크기 x64 | 작은 런처(`rf-open.exe`, 1MB 미만, 시작 20ms 미만) | | | 미측정 | |
| C1-2 런처 크기 ARM64(빌드만) | 작은 런처(`rf-open.exe`, 1MB 미만, 시작 20ms 미만) | | | 미측정 | |
| C1-3 시작(본체 실행 중, p95) | 작은 런처(`rf-open.exe`, 1MB 미만, 시작 20ms 미만) | | | 미측정 | |
| C1-4 본체 미실행 → 본체 기동 | (2) 실행 중이 아니면 본체를 띄우며 | | | 미측정 | |
| C1-5 본체 무응답 → 탐색기 | (3) 본체가 3초(설정 가능) 안에 응답하지 않거나 실행에 실패하면 explorer.exe로 같은 경로를 연다 | | | 미측정 | |
| C1-6 본체 실행 실패 → 탐색기 | (3) 본체가 3초(설정 가능) 안에 응답하지 않거나 실행에 실패하면 explorer.exe로 같은 경로를 연다 | | | 미측정 | |
| C1-7 특수 경로 6종 전달(RF1) | 어떤 경우에도 폴더가 안 열리는 상황은 없어야 한다 | | | 미측정 | |
| C1-8 폴백이 런처로 되돌아오지 않음 | 어떤 경우에도 폴더가 안 열리는 상황은 없어야 한다 | | | 미측정 | |

## C2. 진입 경로 (WINX-02, Win+E는 C3, `/select`는 C4)

판정 규칙: 측정값에 등록 변형 V1·V2·V3 각각의 결과를 `V1:✔ V2:✔ V3:✘`처럼 적고, 기준을 만족하는 변형이 하나라도 있으면 PASS.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C2-1 바탕화면·다른 앱에서 폴더·드라이브 더블클릭(ShellExecute) | Racoon Finder | | | 미측정 | |
| C2-2 바탕화면의 '내 PC', '휴지통', '네트워크' 아이콘 | Racoon Finder | | | 미측정 | |
| C2-3 다른 앱의 "폴더 열기"(ShellExecute `open`/`explore`) | Racoon Finder | | | 미측정 | |
| C2-4 실행 창(Win+R)에 폴더 경로 입력 | Racoon Finder | | | 미측정 | |
| C2-5 제어판, 설정, 프린터 등 그 밖의 셸 가상 폴더 | 탐색기로 넘긴다 | | | 미측정 | |
| C2-6 등록 되돌리기 | 바꾸기 전 값은 변경 원장(INST-02)에 기록하고, 끄거나 삭제하면 원래대로 돌린다 | | | 미측정 | |

## C3. Win+E 가로채기 (WINX-07)

판정 규칙: Task 10의 안정성 점검 a~f를 모두 통과하면 그 방식은 PASS.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C3-1 방식 A: 레지스트리 재정의(CLSID opennewwindow) | 레지스트리 재정의를 우선 쓰고, 안 되면 키보드 훅을 쓴다. 어느 방식이 Windows 11에서 안정적으로 동작하는지는 M0에서 검증한다. | | | 미측정 | |
| C3-2 방식 B: DisabledHotkeys + RegisterHotKey | 레지스트리 재정의를 우선 쓰고, 안 되면 키보드 훅을 쓴다. 어느 방식이 Windows 11에서 안정적으로 동작하는지는 M0에서 검증한다. | | | 미측정 | |
| C3-3 방식 C: 저수준 키보드 훅 | 레지스트리 재정의를 우선 쓰고, 안 되면 키보드 훅을 쓴다. 어느 방식이 Windows 11에서 안정적으로 동작하는지는 M0에서 검증한다. | | | 미측정 | |

## C4. `/select` 가로채기 (WINX-02)

판정 규칙: 측정값에 방식 M1(레지스트리 변형)과 M2(창 가로채기)의 결과와 M2의 `flash_ms` 중앙값을 적는다. 폴더와 선택 항목이 맞게 앱에 오면 PASS, 탐색기가 열리면 FAIL.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C4-1 `explorer.exe /select,<파일>` | 최선 노력. 방식은 M0에서 검증하고, 실패하면 탐색기가 열린다. | | | 미측정 | |
| C4-2 `SHOpenFolderAndSelectItems` | 최선 노력. 방식은 M0에서 검증하고, 실패하면 탐색기가 열린다. | | | 미측정 | |
| C4-3 Chrome 다운로드 "폴더에서 보기" | 최선 노력. 방식은 M0에서 검증하고, 실패하면 탐색기가 열린다. | | | 미측정 | |
| C4-4 Edge 다운로드 "폴더에 표시" | 최선 노력. 방식은 M0에서 검증하고, 실패하면 탐색기가 열린다. | | | 미측정 | |
| C4-5 VS Code "Reveal in File Explorer" | 최선 노력. 방식은 M0에서 검증하고, 실패하면 탐색기가 열린다. | | | 미측정 | |

## C5. 복사 엔진 (PERF-07, PERF-08)

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C5-1 작은 파일, Windows | 탐색기·Finder 소요 시간의 50% 이하 | | | 미측정 | |
| C5-2 작은 파일, macOS | 탐색기·Finder 소요 시간의 50% 이하 | | | 미측정 | |
| C5-3 10GB 파일, Windows | 대상 장치 순차 쓰기 대역폭의 95% 이상 | | | 미측정 | |
| C5-4 10GB 파일, macOS | 대상 장치 순차 쓰기 대역폭의 95% 이상 | | | 미측정 | |
| C5-5 비정렬 크기 무결성(RF2) | 복사·이동·동기화·이름 변경 중 데이터 손실이 없어야 한다 | | | 미측정 | |

## C6. OpenDAL (함께 검증, R9)

판정 규칙: C6-6~C6-11은 그 공급자의 PRD 요구를 OpenDAL로 모두 채우면 PASS(유지), 하나라도 못 채우면 FAIL(직접 구현 후보)이고 메모에 빠진 기능을 적는다.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C6-1 SFTP 목록 | OpenDAL로 SFTP·OneDrive 목록 조회와 업로드 | | | 미측정 | |
| C6-2 SFTP 업로드 | OpenDAL로 SFTP·OneDrive 목록 조회와 업로드 | | | 미측정 | |
| C6-3 SFTP Windows 빌드 | Windows 11 x64, ARM64 | | | 미측정 | |
| C6-4 OneDrive 목록 | OpenDAL로 SFTP·OneDrive 목록 조회와 업로드 | | | 미측정 | |
| C6-5 OneDrive 업로드(16KB, 300MB) | OpenDAL로 SFTP·OneDrive 목록 조회와 업로드 | | | 미측정 | |
| C6-6 FTP·FTPS | FTP, FTPS(명시적·암시적 TLS) … FTP 수동·능동 모드를 고르고, 서버 파일명 인코딩(CP949 등)을 지정할 수 있다 | | | 미측정 | |
| C6-7 SFTP 인증·호스트 키 | SFTP는 비밀번호, 키 파일, SSH 에이전트 인증과 호스트 키 확인(지문 저장)을 지원한다 | | | 미측정 | |
| C6-8 WebDAV | 기본 인증과 다이제스트 인증을 지원한다 | | | 미측정 | |
| C6-9 Google Drive | 공유 드라이브를 지원한다 … 실제 파일이 없는 클라우드 문서는 바로가기로 표시한다 | | | 미측정 | |
| C6-10 OneDrive(개인, 회사·학교) | 지원하는 프로토콜(SFTP, FTP REST, 클라우드 업로드 세션)에서는 이어 받기·올리기를 한다 | | | 미측정 | |
| C6-11 Dropbox | 지원하는 프로토콜(SFTP, FTP REST, 클라우드 업로드 세션)에서는 이어 받기·올리기를 한다 | | | 미측정 | |
| C6-12 SFTP 대체 후보 russh-sftp(참고) | 부족한 공급자는 그 공급자만 직접 구현한다 | | | 미측정 | |

## C7. 변경 원장과 VM 스냅샷 비교 (INST-02, INST-07)

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C7-1 Windows 설치→사용→삭제 | 레지스트리와 파일 시스템 스냅샷을 비교해 차이가 0이어야 한다 | | | 미측정 | |
| C7-2 Windows 양성 대조(RF3) | 레지스트리와 파일 시스템 스냅샷을 비교해 차이가 0이어야 한다 | | | 미측정 | |
| C7-3 macOS 설치→사용→삭제 | 레지스트리와 파일 시스템 스냅샷을 비교해 차이가 0이어야 한다 | | | 미측정 | |
| C7-4 macOS 양성 대조(RF3) | 레지스트리와 파일 시스템 스냅샷을 비교해 차이가 0이어야 한다 | | | 미측정 | |
| C7-5 허용 목록 | OS가 스스로 관리하는 항목(Prefetch, 이벤트 로그 등)은 문서화한 허용 목록으로만 제외한다 | | | 미측정 | |
| C7-6 CI 자동화 가능성(참고) | CI에서 깨끗한 Windows 11과 macOS 15 VM에 설치하고, 기능 사용 시나리오를 돌린 뒤 삭제한다 | | | 미측정 | |

## C8. Windows 설치 도구 (Q7)

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C8-1 사용자 단위 설치 | 사용자 단위 설치가 기본이다(관리자 권한 불필요, `%LOCALAPPDATA%\Programs\RacoonFinder`) | | | 미측정 | |
| C8-2 제거 순서와 원장 복원 | (1) 앱, 상주 프로세스, 색인 프로세스 종료, (2) 원장을 역순으로 되돌림 … (4) 설정·캐시·로그 … 삭제, (5) 설치 폴더와 제거 프로그램 등록 삭제 | | | 미측정 | |
| C8-3 색인 서비스만 관리자 승인 | 빠른 파일 검색의 색인 서비스(IDX-05)를 고르면 설치 중 관리자 승인을 한 번 받는다 | | | 미측정 | |
| C8-4 제거 시 서비스 해제 승인 | 서비스가 설치돼 있으면 이 단계에서 관리자 승인을 받는다 | | | 미측정 | |
| C8-5 "삭제 전에 설정 백업" 체크박스 | 시작 화면에 "삭제 전에 설정 백업" 체크박스(기본 해제)가 있고, 체크하면 백업 파일 하나만 문서 폴더에 남긴다 | | | 미측정 | |
| C8-6 라이선스·ARM64(참고) | Inno Setup(사용자 단위 설치, 관리자 불필요). 원장 복원은 앱이 담당 | | | 미측정 | |

## C9. NTFS 색인 속도 (PERF-13, IDX-02, IDX-03)

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C9-1 MFT 직접 읽기, 100만 | NTFS 고속 색인 10초 이하 | | | 미측정 | |
| C9-2 FSCTL_ENUM_USN_DATA, 100만(참고) | NTFS 고속 색인 10초 이하 | | | 미측정 | |
| C9-3 폴더 훑기, 100만 | 폴더 훑기 방식과 macOS는 3분 이하 | | | 미측정 | |
| C9-4 항목 집합 일치(RF4) | 항목마다 이름, 경로, 크기, 수정·생성 날짜, 속성을 저장한다 | | | 미측정 | |
| C9-5 USN 변경 1만 건 반영(참고) | 파일 생성·삭제·이름 변경이 검색 결과에 반영되기까지 2초 이하 | | | 미측정 | |

## C10. macOS 색인 (PERF-13, IDX-04)

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C10-1 getattrlistbulk, 100만 이미지 | 폴더 훑기 방식과 macOS는 3분 이하 | | | 미측정 | |
| C10-2 전체 디스크 훑기(참고) | macOS 전체 디스크 훑기 속도 | | | 미측정 | |
| C10-3 `searchfs` 사용 가능성 | 더 빠른 초기 색인 방법(카탈로그 검색 `searchfs`)이 APFS에서 쓸 만한지는 M0에서 확인한다 | | | 미측정 | |
| C10-4 FSEvents 이력 따라잡기 | 색인 프로세스가 꺼져 있던 동안의 변경은 FSEvents 이력(마지막으로 처리한 이벤트 ID 이후)으로 따라잡는다 | | | 미측정 | |
| C10-5 재부팅 후 따라잡기 | 색인 프로세스가 꺼져 있던 동안의 변경은 FSEvents 이력(마지막으로 처리한 이벤트 ID 이후)으로 따라잡는다 | | | 미측정 | |
| C10-6 펌링크 중복 0(RF4) | 펌링크(`/System/Volumes/Data`) 때문에 같은 파일이 두 경로로 잡히지 않게 한다 | | | 미측정 | |
| C10-7 항목 집합 일치(RF4) | 항목마다 이름, 경로, 크기, 수정·생성 날짜, 속성을 저장한다 | | | 미측정 | |

## C11. ZIP(AES)·7z 쓰기 라이브러리

판정 규칙: 측정값에 후보별 결과를 `zip:✔ sevenz-rust2:— bsdtar:✘`처럼 적는다(✔ 지원·검증, ✘ 미지원·실패, — 해당 없음). 판정은 그 행을 채우는 후보가 하나라도 있으면 PASS.

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C11-1 ZIP AES-256 쓰기(7-Zip 검증) | ZIP(AES-256 암호, 압축 수준 선택) | | | 미측정 | |
| C11-2 ZIP64(4.5GB 항목) | ZIP(ZIP64, AES·ZipCrypto 암호) | | | 미측정 | |
| C11-3 ZIP UTF-8 이름 플래그 | Windows에서 만든 ZIP의 한글 파일명(CP949)이 macOS나 다른 압축 프로그램에서 깨진다 | | | 미측정 | |
| C11-4 7z LZMA2 + 암호 | 7z(LZMA2, 암호, 파일명 암호화) | | | 미측정 | |
| C11-5 7z 파일명 암호화 | 7z(LZMA2, 암호, 파일명 암호화) | | | 미측정 | |
| C11-6 분할 크기 지정 | 분할 크기를 지정할 수 있다 | | | 미측정 | |
| C11-7 기존 압축 파일에 추가·삭제·이름 변경 | 기존 ZIP·7z 안으로 끌어다 놓으면 파일이 추가된다. 내부 항목의 삭제와 이름 변경도 된다. | | | 미측정 | |
| C11-8 처리 시간(작은 파일 1만 개, 참고) | 압축 수준 선택 | | | 미측정 | |

## C12. NSFileViewer 앱 호환성 (MACX-01, R5)

판정 규칙: 요청이 앱 로그에 오면 PASS, Finder가 열리면 FAIL, 아무것도 안 열리면 FAIL(메모에 "무반응"). 설치되지 않은 앱은 `미측정`(메모 "미설치").

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C12-1 Terminal `open -R` | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-2 Safari 다운로드 | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-3 Chrome 다운로드 | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-4 Firefox 다운로드 | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-5 Mail 첨부 | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-6 VS Code "Reveal in Finder" | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-7 Xcode "Show in Finder" | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-8 Spotlight "Finder에서 보기" | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-9 미리보기 제목 막대 | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-10 TextEdit 제목 막대 | 다른 앱의 "Finder에서 보기" 요청이 Racoon Finder로 온다 | | | 미측정 | |
| C12-11 설정 전부터 실행 중이던 앱 | macOS `NSFileViewer` 방식이 앱별로, 또는 재부팅 후 불안정함 | | | 미측정 | |
| C12-12 재부팅 후 | macOS `NSFileViewer` 방식이 앱별로, 또는 재부팅 후 불안정함 | | | 미측정 | |
| C12-13 되돌리기 | 끄거나 삭제하면 원래 값으로 돌린다 | | | 미측정 | |

## C13. 의존성 라이선스와 최소 Rust 버전 (8.5)

| 항목 | 기준(PRD 값 그대로) | 측정값 | 환경 | 판정 | 메모 |
|---|---|---|---|---|---|
| C13-1 스파이크 전체 cargo-deny licenses | M0에서 cargo-deny로 전체 의존성 트리의 라이선스를 검사하고 | | | 미측정 | |
| C13-2 제품 후보 크레이트 rust-version(참고) | 언어: Rust(stable) | | | 미측정 | |

## D. 결정 표

| 결정 | 선택 | 근거(보고서 행) | 탈락한 대안과 이유 | PRD 반영 위치 |
|---|---|---|---|---|
| D1 UI 프레임워크와 렌더러 | | | | 8.1 UI, 8.4 UI 프레임워크, 12 R1, 14 |
| D2 제품 워크스페이스 최소 Rust 버전 | | | | 8.1 언어 |
| D3 폴더 열기 등록 방식(V1/V2/V3) | | | | 8.4, 5.16 WINX-06 |
| D4 Win+E 방식 | | | | 8.4, 5.16 WINX-07, 12 R4 |
| D5 `/select` 지원 여부와 방식 | | | | 8.4, 5.16 WINX-02 표 |
| D6 ZIP(AES) 쓰기 라이브러리 | | | | 8.1 압축, 8.4, 8.5 |
| D7 7z 쓰기 라이브러리 | | | | 8.1 압축, 8.4, 8.5 |
| D8 Windows 설치 도구 | | | | 8.1 설치, 8.4, 13 Q7 |
| D9 NTFS 초기 색인 방식 | | | | 8.1 파일 색인, 8.4 |
| D10 macOS 초기 색인 방식 | | | | 8.1 파일 색인, 8.4, 5.11 IDX-04, 12 R16 |
| D11 OpenDAL에서 직접 구현으로 바꿀 공급자 | | | | 8.1 원격 저장소, 8.4, 8.5, 12 R9 |
| D12 OS 드래그 앤 드롭 구현 계층 | | | | 8.4 |
| D13 NSFileViewer 대체 경로 필요 여부 | | | | 12 R5, 5.17 MACX-01 |
````

- [ ] **Step 9: 커밋한다**

```bash
git add spikes/README.md spikes/.gitignore spikes/dataset-gen docs/spikes/m0-report.md
git commit -m "chore(spike): add M0 report skeleton and dataset generator"
```

---

### Task 2: [Windows 실기 필요] Windows 기준 환경과 데이터셋 준비

**Files:**
- Modify: `docs/spikes/m0-report.md`(1장 E-W1·E-W2, 2장 Windows 열)

**Interfaces:**
- Consumes: Task 1의 `rfspike-data` CLI와 볼륨 스크립트.
- Produces: `%RFSPIKE_DATA%`·`%RFSPIKE_DST%` 환경 변수, 데이터셋 6종(Windows), VHDX `R:\`(8.3 이름 켬), `C:\tools\RAMMap64.exe`. Task 4~11, 14, 15, 19가 쓴다.

- [ ] **Step 1: 도구를 설치한다**

rustup(MSVC 툴체인), Visual Studio Build Tools의 "C++를 사용한 데스크톱 개발" 워크로드, Git을 설치한다. Sysinternals RAMMap(https://learn.microsoft.com/sysinternals/downloads/rammap)을 `C:\tools\RAMMap64.exe`에 둔다.

Run(관리자 PowerShell): `rustc --version; C:\tools\RAMMap64.exe -accepteula -Et; $LASTEXITCODE`
Expected: `rustc 1.` 로 시작하는 줄과 `0`.

- [ ] **Step 2: 서로 다른 NVMe 두 개를 데이터 위치로 정한다**

원본·대상 드라이브 문자를 고른다(예: D, E). Run: `Get-Partition -DriveLetter D,E | Get-Disk | Select-Object Number,FriendlyName,BusType`
Expected: `Number`가 서로 다른 두 줄, 둘 다 `BusType`이 `NVMe`. 그다음 `setx RFSPIKE_DATA D:\rfspike-data`, `setx RFSPIKE_DST E:\rfspike-dst`를 실행하고 새 PowerShell을 연다.

- [ ] **Step 3: 환경을 기록한다**

Run: `cargo run --release --manifest-path spikes\dataset-gen\Cargo.toml -- env`
Expected: `ENV os="Windows 11` 로 시작하는 한 줄. 이 줄을 보고서 E-W1에 붙이고, 메모에 Step 2의 디스크 모델·버스, 설정 > 디스플레이의 주사율·배율, `winver`의 OS 빌드를 적는다. ARM64 장비가 있으면 같은 방법으로 E-W2를 채우고, 없으면 E-W2 메모에 "장비 없음"을 적는다.

- [ ] **Step 4: 일반 데이터셋을 만들고 검증한다**

```powershell
$B = ".\spikes\dataset-gen\target\release\rfspike-data.exe"
& $B big-folder --out $env:RFSPIKE_DATA;  & $B verify --root "$env:RFSPIKE_DATA\big-folder"
& $B small-files --out $env:RFSPIKE_DATA; & $B verify --root "$env:RFSPIKE_DATA\small-files"
& $B large-file --out $env:RFSPIKE_DATA;  & $B verify --root "$env:RFSPIKE_DATA\large-file"
& $B thousand --out $env:RFSPIKE_DATA;    & $B verify --root "$env:RFSPIKE_DATA\thousand"
& $B fixtures --out $env:RFSPIKE_DATA
```
Expected: 네 줄 `VERIFY ok kind=big-folder entries=100000`, `VERIFY ok kind=small-files entries=10100`, `VERIFY ok kind=large-file entries=1`, `VERIFY ok kind=thousand entries=1000`.

- [ ] **Step 5: VHDX 안에 100만 개 데이터셋을 만든다**

Run(관리자 PowerShell): `.\spikes\dataset-gen\scripts\win-make-vhdx.ps1` 다음 `& $B million --out R:\ --threads 8; & $B verify --root R:\`
Expected: `fsutil 8dot3name query R:` 출력에 상태 값 `0`, 마지막 줄 `VERIFY ok kind=million entries=1011101`.

- [ ] **Step 6: 보고서 2장의 Windows 열을 채우고 커밋한다**

`VERIFY` 결과 열에 각 데이터셋의 `paths_xxh3`와 생성 소요 시간을 적는다.

```bash
git add docs/spikes/m0-report.md
git commit -m "docs(spike): record Windows baseline environment and datasets"
```

---

### Task 3: [macOS 실기 필요] macOS 기준 환경과 데이터셋 준비

**Files:**
- Modify: `docs/spikes/m0-report.md`(1장 E-M1, 2장 macOS 열)

**Interfaces:**
- Consumes: Task 1의 `rfspike-data` CLI와 볼륨 스크립트.
- Produces: `$RFSPIKE_DATA`·`$RFSPIKE_DST`, 데이터셋 6종(macOS), `/Volumes/RFSPIKE1M`. Task 4~6, 8, 12, 14, 16, 20이 쓴다.

- [ ] **Step 1: 도구를 설치한다**

`xcode-select --install`, rustup(aarch64-apple-darwin). Run: `rustc --version && sudo -v && sudo -n purge; echo $?`
Expected: `rustc 1.` 로 시작하는 줄과 `0`.

- [ ] **Step 2: 데이터 위치를 정한다**

원본은 내장 SSD의 `~/rfspike-data`, 대상은 외장 NVMe(USB4·Thunderbolt 케이스)의 `/Volumes/<외장>/rfspike-dst`. `~/.zshrc`에 `export RFSPIKE_DATA=~/rfspike-data`, `export RFSPIKE_DST=/Volumes/<외장>/rfspike-dst`를 넣고 새 셸을 연다. 외장 NVMe가 없으면 `RFSPIKE_DST`를 내장 SSD의 별도 APFS 볼륨(`diskutil apfs addVolume disk3 APFS rfspike-dst`)으로 두고, E-M1 메모에 "대상이 같은 물리 장치"를 적는다(이 경우 C5-2·C5-4 판정은 `참고`).

- [ ] **Step 3: 환경을 기록한다**

Run: `cargo run --release --manifest-path spikes/dataset-gen/Cargo.toml -- env` 와 `system_profiler SPDisplaysDataType SPNVMeDataType | grep -E "Resolution|Refresh|Model"`
Expected: `ENV os="macOS 15` 로 시작하는 줄. E-M1에 붙이고 메모에 화면 주사율·디스크 모델·`sw_vers -buildVersion`을 적는다.

- [ ] **Step 4: 일반 데이터셋을 만들고 검증한다**

```zsh
B=./spikes/dataset-gen/target/release/rfspike-data
for k in big-folder small-files large-file thousand; do $B $k --out "$RFSPIKE_DATA" && $B verify --root "$RFSPIKE_DATA/$k"; done
$B fixtures --out "$RFSPIKE_DATA"
```
Expected: Task 2 Step 4와 같은 `VERIFY ok` 네 줄.

- [ ] **Step 5: 희소 APFS 이미지 안에 100만 개 데이터셋을 만든다**

Run: `./spikes/dataset-gen/scripts/mac-make-image.sh && $B million --out /Volumes/RFSPIKE1M --threads 8 && $B verify --root /Volumes/RFSPIKE1M`
Expected: `VERIFY ok kind=million entries=1011101`.

- [ ] **Step 6: 보고서 2장의 macOS 열을 채우고 커밋한다**

```bash
git add docs/spikes/m0-report.md
git commit -m "docs(spike): record macOS baseline environment and datasets"
```

---

### Task 4: [Windows 실기 필요] [macOS 실기 필요] 게이트 1·5 대용량 목록 스크롤과 콜드 스타트

**Files:**
- Create: `spikes/slint-list/Cargo.toml`, `build.rs`, `ui/app.slint`, `src/lib.rs`, `src/stats.rs`, `src/main.rs`, `src/bin/coldstart.rs`
- Modify: `docs/spikes/m0-report.md`(G1, G5)

**Interfaces:**
- Consumes: `big-folder`, `thousand` 데이터셋(Task 2·3), 콜드 캐시 명령(전역 제약).
- Produces: `RESULT G1 os=<windows|macos> backend=<b> refresh_hz=<f> phase=<steady|jump> frames=<n> avg_fps=<f> p95_ms=<f> p99_ms=<f> dropped_pct=<f> rows=<n> profile=<p> verdict=<v>`; `RESULT G5 os=<o> backend=<b> cache=<cold|warm> runs=<n> median_ms=<f> min_ms=<f> max_ms=<f> profile=<p> verdict=<v> [reason=<r>]`. 게이트를 통과한 렌더러 이름(G5 메모)을 Task 22 D1이 쓴다.

- [ ] **Step 1: 프로젝트를 만들고 의존성을 추가한다**

```bash
cargo new --vcs none --lib --name rfspike-slint-list spikes/slint-list
cargo add --manifest-path spikes/slint-list/Cargo.toml slint@1.17 -F renderer-skia,renderer-femtovg,renderer-software,unstable-winit-030
cargo add --manifest-path spikes/slint-list/Cargo.toml --build slint-build@1.17
```
`Cargo.toml`에 `[[bin]] name = "rfspike-list" path = "src/main.rs"`와 `[[bin]] name = "rfspike-coldstart" path = "src/bin/coldstart.rs"`를 둔다. `build.rs`는 `slint_build::compile("ui/app.slint")`, `main.rs`는 `slint::include_modules!()`.

- [ ] **Step 2: 판정 로직의 실패하는 테스트를 쓴다**

`src/stats.rs`의 `#[cfg(test)]`:

```rust
#[test]
fn steady_60hz_passes() {
    let t: Vec<f64> = (0..601).map(|i| i as f64 * 16.667).collect();
    let s = frame_stats(&t);
    assert_eq!(s.frames, 600);
    assert!((s.avg_fps - 60.0).abs() < 0.1);
    assert_eq!(s.dropped_pct, 0.0);
    assert!(g1_pass(&s));
}

#[test]
fn two_percent_drops_fail() {
    let mut t = vec![0.0];
    for i in 0..600 { let d = if i % 50 == 0 { 33.333 } else { 16.667 }; t.push(t.last().unwrap() + d); }
    let s = frame_stats(&t);
    assert!((s.dropped_pct - 2.0).abs() < 1e-9);
    assert!(!g1_pass(&s));
}

#[test]
fn g5_median_and_threshold() {
    assert_eq!(g5_summary(&[300.0, 310.0, 500.0], true, true), (310.0, "PASS"));
    assert_eq!(g5_summary(&[401.0, 420.0, 390.0], true, true), (401.0, "FAIL"));
}

#[test]
fn g5_invalid_when_flush_failed_or_debug() {
    assert_eq!(g5_summary(&[300.0], false, true).1, "INVALID");
    assert_eq!(g5_summary(&[300.0], true, false).1, "INVALID");
}
```

Run: `cargo test --release --manifest-path spikes/slint-list/Cargo.toml --lib`
Expected: FAIL, `cannot find function `frame_stats``.

- [ ] **Step 3: 판정 로직을 구현한다**

`src/stats.rs`: `pub struct FrameStats { pub frames: usize, pub avg_fps: f64, pub p95_ms: f64, pub p99_ms: f64, pub dropped_pct: f64 }`, `pub fn frame_stats(frame_times_ms: &[f64]) -> FrameStats`(입력은 프레임 시작 시각, 간격 n-1개로 계산, 25.0ms 초과 간격이 드롭), `pub fn g1_pass(s: &FrameStats) -> bool`(avg_fps ≥ 59.0 && dropped_pct ≤ 1.0), `pub fn g5_summary(runs_ms: &[f64], flush_ok: bool, release: bool) -> (f64, &'static str)`(중앙값, 짝수 개면 아래쪽 중앙값; flush 실패나 debug면 `"INVALID"`, 중앙값 ≤ 400.0이면 `"PASS"`, 아니면 `"FAIL"`).

Run: `cargo test --release --manifest-path spikes/slint-list/Cargo.toml --lib`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 4: 목록 앱과 콜드 스타트 측정기를 만든다**

`rfspike-list --dir <path> [--autoscroll <secs>] [--exit-on-first-frame]`:
- 창 1600×1000 논리 px, `ListView` 안에 높이 24px 행, 열 5개(이름·크기·수정 날짜·종류·확장자). 모델은 `Rc<VecModel<Row>>`.
- 백그라운드 스레드가 `std::fs::read_dir`로 2,000개씩 묶어 `slint::invoke_from_event_loop`로 모델에 붙인다(PRD 8.3 데이터 흐름).
- `window.set_rendering_notifier`의 `RenderingState::BeforeRendering` 시각을 모은다. 렌더러가 notifier를 지원하지 않으면 `verdict=INFO reason=notifier-unsupported`.
- `--autoscroll N`: 모두 불러온 뒤 `slint::Timer`(4ms 반복)로 `ListView`의 `viewport-y`를 바꾼다. steady phase는 N초 동안 3,000px/s, jump phase는 N초 동안 틱마다 시드 42 xorshift로 고른 임의 위치. phase마다 `RESULT G1` 한 줄. `refresh_hz`는 `slint::winit_030::WinitWindowAccessor::with_winit_window`로 얻은 `current_monitor()`의 `refresh_rate_millihertz()/1000`.
- `--exit-on-first-frame`: 첫 묶음이 모델에 들어간 뒤 처음 그린 프레임에서 `FIRST_FRAME rows=<n> profile=<release|debug>`를 출력하고 `slint::quit_event_loop()`.

`rfspike-coldstart --app <exe> --dir <path> --runs <n> --flush <cmd|none>`: 회차마다 flush 명령을 실행(Windows `cmd /C`, macOS `sh -c`), 2초 쉬고, `Instant::now()` 뒤 앱을 `--exit-on-first-frame`으로 띄워 stdout에서 `FIRST_FRAME`을 읽을 때까지의 ms를 잰다. `--flush none`이면 `cache=warm verdict=INFO`. 판정은 `g5_summary`.

Run: `cargo build --release --manifest-path spikes/slint-list/Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 5: Windows에서 게이트 1을 잰다**

```powershell
foreach ($b in "winit-femtovg","winit-skia","winit-software") {
  $env:SLINT_BACKEND = $b
  .\spikes\slint-list\target\release\rfspike-list.exe --dir "$env:RFSPIKE_DATA\big-folder" --autoscroll 10
}
```
Expected: 렌더러마다 `RESULT G1 os=windows backend=<b> ... phase=steady ... rows=100000 profile=release verdict=...`와 `phase=jump` 줄. 창은 측정 내내 주 모니터 앞에 둔다.

- [ ] **Step 6: Windows에서 게이트 5를 재고 RF5를 확인한다**

```powershell
$L = ".\spikes\slint-list\target\release"
foreach ($b in "winit-femtovg","winit-skia","winit-software") {
  $env:SLINT_BACKEND = $b
  & "$L\rfspike-coldstart.exe" --app "$L\rfspike-list.exe" --dir "$env:RFSPIKE_DATA\thousand" --runs 10 --flush "C:\tools\RAMMap64.exe -accepteula -Et"
  & "$L\rfspike-coldstart.exe" --app "$L\rfspike-list.exe" --dir "$env:RFSPIKE_DATA\thousand" --runs 10 --flush none
}
& "$L\rfspike-coldstart.exe" --app "$L\rfspike-list.exe" --dir "$env:RFSPIKE_DATA\thousand" --runs 1 --flush "cmd /c exit 1"
```
Expected: 렌더러마다 `RESULT G5 os=windows ... cache=cold runs=10 ...`와 `cache=warm ... verdict=INFO`, 마지막 줄은 `verdict=INVALID reason=flush-failed`. 재부팅 후 로그인해 2분 기다린 뒤 `--runs 1 --flush none`을 한 번 더 돌려 G5-1 메모에 적는다.

- [ ] **Step 7: macOS에서 같은 측정을 한다**

```zsh
L=./spikes/slint-list/target/release
cargo test --release --manifest-path spikes/slint-list/Cargo.toml --lib && cargo build --release --manifest-path spikes/slint-list/Cargo.toml
for b in winit-femtovg winit-skia winit-software; do SLINT_BACKEND=$b $L/rfspike-list --dir "$RFSPIKE_DATA/big-folder" --autoscroll 10; done
sudo -v
for b in winit-femtovg winit-skia winit-software; do
  SLINT_BACKEND=$b $L/rfspike-coldstart --app $L/rfspike-list --dir "$RFSPIKE_DATA/thousand" --runs 10 --flush "sudo -n purge"
  SLINT_BACKEND=$b $L/rfspike-coldstart --app $L/rfspike-list --dir "$RFSPIKE_DATA/thousand" --runs 10 --flush none
done
```
Expected: `test result: ok. 4 passed`, 그리고 Step 5·6과 같은 형식의 `os=macos` 줄.

- [ ] **Step 8: 기록하고 커밋한다(OS 세션마다)**

G1·G5 판정 규칙대로 행을 채운다. 120Hz 화면이 있으면 G1-5에 그 화면의 avg_fps를 적고, 없으면 메모에 "120Hz 화면 없음".

```bash
git add spikes/slint-list docs/spikes/m0-report.md
git commit -m "docs(spike): record Slint gates 1 and 5 on <windows|macos>"
```

---

### Task 5: [Windows 실기 필요] [macOS 실기 필요] 게이트 2 한글 IME

**Files:**
- Create: `spikes/slint-ime/Cargo.toml`, `build.rs`, `ui/app.slint`, `src/main.rs`
- Modify: `docs/spikes/m0-report.md`(G2)

**Interfaces:**
- Consumes: `fixtures/ime`(Task 1).
- Produces: 로그 줄 `PREEDIT "<text>"`, `EDITED "<text>"`, `ACCEPT "<text>"`, `CANCEL`, `KEY "<text>" editing=<bool>`, `TYPE_TO_SELECT "<text>"`(stdout과 `rfspike-ime.log`).

- [ ] **Step 1: harness를 만든다**

```bash
cargo new --vcs none --name rfspike-slint-ime spikes/slint-ime
cargo add --manifest-path spikes/slint-ime/Cargo.toml slint@1.17
cargo add --manifest-path spikes/slint-ime/Cargo.toml --build slint-build@1.17
```
재는 것: Slint `TextInput`이 winit IME 이벤트를 받아 조합(preedit)을 그리는지, 조합 중 Enter·Esc가 `accepted`·`key-pressed`로 어떻게 오는지. 화면: `fixtures/ime`의 다섯 이름 목록. 바깥 `FocusScope`가 목록 키 입력을 `TYPE_TO_SELECT`로 기록한다(새는 글자 감지용). F2(macOS는 Enter)로 선택 행이 이름 바꾸기 모드가 되며 `TextInput`에 원래 이름을 넣고 확장자 앞까지 선택한다. `TextInput`의 `changed preedit-text => {...}`가 `PREEDIT`, `edited`가 `EDITED`, `accepted`가 `ACCEPT`(행 이름을 바꾸고 모드 종료), `key-pressed`에서 `Key.Escape`면 `CANCEL`(원래 이름 유지, 모드 종료). 모든 `key-pressed`는 `KEY`로 기록하고 `reject`로 넘긴다(Escape만 `accept`).

Run: `cargo build --release --manifest-path spikes/slint-ime/Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 2: Windows에서 T1~T3을 실행한다**

Microsoft 한국어 IME(2벌식)로 `.\spikes\slint-ime\target\release\rfspike-slint-ime.exe --dir "$env:RFSPIKE_DATA\fixtures\ime"`를 띄운다.
- T1(G2-1): `보고서.txt` 행에서 F2 → ㅎ ㅏ ㄴ 입력. 통과 조건: 입력란 커서 위치에 "ㅎ"→"하"→"한"이 보이고, 로그에 `PREEDIT "ㅎ"`, `PREEDIT "하"`, `PREEDIT "한"`이 순서대로 있다. 후보·조합 창이 있으면 입력란 근처에 뜬다.
- T2(G2-2): F2 → "한글" 입력(마지막 "글"이 조합 중) → Enter 한 번. 통과 조건: 로그에 `ACCEPT "한글.txt"`가 정확히 한 번, `TYPE_TO_SELECT` 없음, 목록의 행 이름이 `한글.txt`.
- T3(G2-3): 다른 행에서 F2 → "한글"(조합 중) → Esc 한 번. 통과 조건: `CANCEL`이 정확히 한 번, `ACCEPT`와 `TYPE_TO_SELECT` 없음, 행 이름이 원래대로이고, 다시 F2를 눌렀을 때 입력란에 원래 이름만 있다.
- T4(G2-7 참고): 조합 중 Backspace 한 번과, 조합 중 다른 행 클릭의 결과를 로그와 함께 메모에 적는다.

- [ ] **Step 3: macOS에서 같은 절차를 실행한다**

입력 소스 "한국어 - 2벌식"으로 `./spikes/slint-ime/target/release/rfspike-slint-ime --dir "$RFSPIKE_DATA/fixtures/ime"`를 띄우고, F2 대신 Enter로 이름 바꾸기를 시작해 T1~T4를 반복한다(G2-4~G2-7).

- [ ] **Step 4: 기록하고 커밋한다(OS 세션마다)**

행마다 측정값에 해당 로그 줄을 그대로 붙인다.

```bash
git add spikes/slint-ime docs/spikes/m0-report.md
git commit -m "docs(spike): record Slint gate 2 Korean IME on <windows|macos>"
```

---

### Task 6: [Windows 실기 필요] [macOS 실기 필요] 게이트 3 탐색기·Finder 양방향 드래그 앤 드롭

**Files:**
- Create: `spikes/slint-dnd/Cargo.toml`, `build.rs`, `ui/app.slint`, `src/main.rs`, `src/win_dnd.rs`, `src/mac_dnd.rs`, `assets/drop-test.html`
- Modify: `docs/spikes/m0-report.md`(G3)

**Interfaces:**
- Consumes: `fixtures/dnd`(Task 1).
- Produces: 로그 줄 `DROP_IN method=<winit|ole|nsview> paths=[...] allowed=<COPY|MOVE|LINK 조합> chosen=<...>`, `DRAG_OUT method=<ole|nsview> items=<n> effect=<COPY|MOVE|LINK|NONE>`. 결과가 D12(구현 계층)의 근거다.

- [ ] **Step 1: 의존성을 추가한다**

```bash
cargo new --vcs none --name rfspike-slint-dnd spikes/slint-dnd
cargo add --manifest-path spikes/slint-dnd/Cargo.toml slint@1.17 -F raw-window-handle-06,unstable-winit-030
cargo add --manifest-path spikes/slint-dnd/Cargo.toml --build slint-build@1.17
cargo add --manifest-path spikes/slint-dnd/Cargo.toml raw-window-handle
cargo add --manifest-path spikes/slint-dnd/Cargo.toml windows --target 'cfg(target_os = "windows")' -F Win32_Foundation,Win32_System_Com,Win32_System_Ole,Win32_System_SystemServices,Win32_UI_Shell,Win32_UI_Shell_Common,Win32_UI_WindowsAndMessaging
cargo add --manifest-path spikes/slint-dnd/Cargo.toml objc2 --target 'cfg(target_os = "macos")'
cargo add --manifest-path spikes/slint-dnd/Cargo.toml objc2-foundation --target 'cfg(target_os = "macos")' -F NSArray,NSString,NSURL
cargo add --manifest-path spikes/slint-dnd/Cargo.toml objc2-app-kit --target 'cfg(target_os = "macos")' -F NSApplication,NSDragging,NSDraggingItem,NSDraggingSession,NSEvent,NSPasteboard,NSResponder,NSView,NSWindow
```

- [ ] **Step 2: harness를 만든다**

배경: Slint 1.18.1의 winit 백엔드는 `start_drag`를 구현하지 않고 winit의 `DroppedFile`도 처리하지 않는다(소스 `i-slint-backend-winit-1.18.1`에 drag 처리 없음). 그래서 OS 계층에서 직접 구현한다(게이트 기준의 "OS 계층 직접 구현 포함").
- 창: 왼쪽 `fixtures/dnd` 목록(행마다 `TouchArea`), 오른쪽 로그. 창 핸들은 `slint::Window::window_handle()`의 raw-window-handle 0.6 값(`Win32` hwnd / `AppKit` ns_view).
- 드롭 받기 방식 winit(두 OS): `slint::winit_030::WinitWindowAccessor::on_winit_window_event`에서 `WindowEvent::DroppedFile(path)`를 모아 `DROP_IN method=winit`.
- 드롭 받기 방식 ole(Windows): `RevokeDragDrop(hwnd)`로 winit의 대상을 떼고 `#[windows::core::implement(IDropTarget)]` 구현을 `RegisterDragDrop`. `DragEnter`·`DragOver`에서 허용 효과(`pdwEffect`)와 Ctrl·Shift·Alt 상태로 효과를 고르고, `Drop`에서 `CF_HDROP`을 `DragQueryFileW`로 읽어 `DROP_IN method=ole`. 실행 인자 `--drop ole|winit`로 고른다.
- 드롭 받기 방식 nsview(macOS): 내용 뷰 위에 `define_class!`로 만든 투명 하위 뷰를 덮고 `hitTest:`는 nil을 돌려 마우스를 통과시키며 `registerForDraggedTypes`(파일 URL)를 건다. `draggingEntered:`가 `draggingSourceOperationMask`와 보조키로 동작을 고르고, `performDragOperation:`이 파일 URL을 읽어 `DROP_IN method=nsview`. `--drop nsview|winit`.
- 끌어내기(Windows): 행에서 포인터가 `GetSystemMetrics(SM_CXDRAG)` 이상 움직이면 경로마다 `SHParseDisplayName` → `SHCreateShellItemArrayFromIDLists` → `BindToHandler(BHID_DataObject)`로 `IDataObject`를 얻고, `#[implement(IDropSource)]`(Esc면 취소, 왼쪽 버튼을 떼면 드롭, 기본 커서) 와 함께 `DoDragDrop(..., DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, ...)`. 반환 효과를 `DRAG_OUT method=ole`.
- 끌어내기(macOS): 같은 조건에서 `NSApplication::sharedApplication(mtm).currentEvent()`를 이벤트로, `NSURL::fileURLWithPath`를 pasteboard writer로 한 `NSDraggingItem` 배열과 `NSDraggingSource` 구현 객체(`draggingSession:sourceOperationMaskForDraggingContext:`는 Copy|Move|Link|Generic, `draggingSession:endedAtPoint:operation:`이 로그)로 `beginDraggingSessionWithItems:event:source:`. `DRAG_OUT method=nsview`.
- `assets/drop-test.html`: 외부 리소스 없이 드롭된 `File`의 `name`과 `size`를 목록으로 보여 주는 정적 페이지.

Run: `cargo build --release --manifest-path spikes/slint-dnd/Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 3: Windows에서 양방향을 시험한다**

`.\spikes\slint-dnd\target\release\rfspike-slint-dnd.exe --dir "$env:RFSPIKE_DATA\fixtures\dnd" --drop winit`로 띄운다.
- G3-1: 탐색기에서 픽스처 5개(`보고서 한글.txt`, `photo 🦝.jpg`, `100% & co.txt`, `폴더 A`, `long\...\file.txt`)를 앱 창에 놓는다. 통과 조건: `DROP_IN`의 경로 5개가 원본 경로와 문자 단위로 같다. `--drop ole`로 다시 띄워 Ctrl·Shift를 누른 드롭마다 `chosen`이 바뀌는지 G3-6 메모에 적는다.
- G3-2: 앱에서 같은 5개를 `%RFSPIKE_DST%\dnd-out`을 연 탐색기 창에 끌어 놓는다(그냥, Shift, Ctrl 각각 새 폴더에). 통과 조건: 각 파일이 도착하고 `(Get-FileHash 원본).Hash -eq (Get-FileHash 대상).Hash`가 `True`, `DRAG_OUT effect`가 탐색기가 한 동작과 일치.
- G3-5: `assets/drop-test.html`을 Edge와 Chrome으로 열어 앱에서 `보고서 한글.txt`를 끌어 놓는다. 페이지에 이름과 크기가 보이면 통과.

- [ ] **Step 4: macOS에서 양방향을 시험한다**

`./spikes/slint-dnd/target/release/rfspike-slint-dnd --dir "$RFSPIKE_DATA/fixtures/dnd" --drop winit`(그다음 `--drop nsview`)로 Step 3과 같은 절차를 Finder와 Safari·Chrome으로 반복한다(G3-3, G3-4, G3-5, G3-6). 해시 비교는 `shasum -a 256 <원본> <대상>`의 두 값이 같으면 통과.

- [ ] **Step 5: 기록하고 커밋한다(OS 세션마다)**

측정값에 사용한 방식과 로그 줄을 적고, 실패한 방식은 메모에 증상을 적는다.

```bash
git add spikes/slint-dnd docs/spikes/m0-report.md
git commit -m "docs(spike): record Slint gate 3 OS drag and drop on <windows|macos>"
```

---

### Task 7: [Windows 실기 필요] 게이트 4 Windows 네이티브 셸 컨텍스트 메뉴

**Files:**
- Create: `spikes/slint-shellmenu/Cargo.toml`, `build.rs`, `ui/app.slint`, `src/main.rs`, `src/shell_menu.rs`
- Modify: `docs/spikes/m0-report.md`(G4)

**Interfaces:**
- Consumes: `fixtures/menu`(Task 1), Task 6에서 확인한 hwnd 얻는 방법.
- Produces: 로그 줄 `MENU shown items=<n> top_level=[...]`, `SUBMENU "<이름>" items=<n>`, `INVOKE verb="<verb>" id=<n> ok=<bool>`, `RENAME requested path="<p>"`.

- [ ] **Step 1: 의존성을 추가한다**

```bash
cargo new --vcs none --name rfspike-slint-shellmenu spikes/slint-shellmenu
cargo add --manifest-path spikes/slint-shellmenu/Cargo.toml slint@1.17 -F raw-window-handle-06
cargo add --manifest-path spikes/slint-shellmenu/Cargo.toml --build slint-build@1.17
cargo add --manifest-path spikes/slint-shellmenu/Cargo.toml raw-window-handle
cargo add --manifest-path spikes/slint-shellmenu/Cargo.toml windows --target 'cfg(target_os = "windows")' -F Win32_Foundation,Win32_System_Com,Win32_UI_Shell,Win32_UI_Shell_Common,Win32_UI_WindowsAndMessaging,Win32_Graphics_Gdi
```

- [ ] **Step 2: harness를 만든다**

`shell_menu.rs`의 `pub fn show_shell_menu(hwnd: HWND, paths: &[PathBuf], screen_pt: POINT, extended: bool) -> windows::core::Result<()>`와 `pub fn show_background_menu(hwnd: HWND, folder: &Path, screen_pt: POINT) -> windows::core::Result<()>`:
1. 경로마다 `SHParseDisplayName` → `SHBindToParent`로 부모 `IShellFolder`와 자식 PIDL(모두 같은 부모).
2. `IShellFolder::GetUIObjectOf(hwnd, 자식들, IContextMenu)`, 가능하면 `IContextMenu3`로 캐스트. 배경 메뉴는 폴더의 `IShellFolder::CreateViewObject(hwnd, IContextMenu)`.
3. `CreatePopupMenu` → `QueryContextMenu(hmenu, 0, 1, 0x7FFF, CMF_NORMAL | CMF_EXPLORE | CMF_CANRENAME | (extended이면 CMF_EXTENDEDVERBS))`.
4. 메뉴가 열려 있는 동안 `SetWindowSubclass`로 hwnd를 서브클래싱해 `WM_INITMENUPOPUP`, `WM_DRAWITEM`, `WM_MEASUREITEM`, `WM_MENUCHAR`를 `IContextMenu3::HandleMenuMsg2`(없으면 `IContextMenu2::HandleMenuMsg`)로 넘긴다. `--no-forward` 인자로 이 전달을 끌 수 있게 해 하위 메뉴 차이를 비교한다.
5. `TrackPopupMenuEx(hmenu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, hwnd, None)` → id가 0이 아니면 `GetCommandString(id-1, GCS_VERBW)`로 동사를 얻는다. 동사가 `rename`이면 `RENAME requested`만 기록하고, 아니면 `CMINVOKECOMMANDINFOEX`(`CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE`, `lpVerb`는 `id-1`의 MAKEINTRESOURCE, `nShow = SW_SHOWNORMAL`)로 `InvokeCommand`.
6. `--app-menu-on-top`: 루트 팝업에 앱 항목 "이동…", "복사…"를 먼저 넣고, 셸 메뉴를 하위 메뉴 "더 보기"로 붙인다(G4-7).

Slint 쪽: 목록 행의 `TouchArea`가 오른쪽 클릭에서 화면 좌표를 넘겨 Rust 콜백을 부르고, 빈 곳 오른쪽 클릭은 배경 메뉴를 부른다. 다중 선택은 Ctrl+클릭.

Run: `cargo build --release --manifest-path spikes/slint-shellmenu/Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 3: 메뉴를 시험한다**

7-Zip과 VS Code를 설치한 상태에서 `.\spikes\slint-shellmenu\target\release\rfspike-slint-shellmenu.exe --dir "$env:RFSPIKE_DATA\fixtures\menu"`를 띄운다.
- G4-1: `a.txt` 오른쪽 클릭. 통과 조건: 커서 위치에 메뉴가 뜨고 `MENU shown`의 항목 목록이 같은 파일의 탐색기 "추가 옵션 표시" 메뉴와 같다.
- G4-2: `a.txt`, `b.txt`, `c.zip` 세 개 선택 후 오른쪽 클릭. 통과 조건: 메뉴가 뜨고 7-Zip 항목이 있다.
- G4-3: "보내기"와 "연결 프로그램"을 펼친다. 통과 조건: `SUBMENU "보내기" items=` 값이 1 이상이고 항목이 보인다. `--no-forward`로 다시 띄워 차이를 메모에 적는다.
- G4-4: "속성"을 고르면 OS 속성 창이 열리고 `INVOKE ok=true`; `b.txt`에 "삭제"를 고르면 휴지통으로 가고 `INVOKE ok=true`; "이름 바꾸기"는 `RENAME requested`.
- G4-5: 메뉴에 VS Code "Code(으)로 열기", "터미널에서 열기", 메모장 "메모장에서 편집" 등이 보이는지 적는다.
- G4-6: 빈 곳 오른쪽 클릭 → "새로 만들기" → "텍스트 문서". 새 파일이 생기면 통과.
- G4-7: `--app-menu-on-top`으로 띄워 맨 위가 "이동…", "복사…"이고 "더 보기" 하위 메뉴가 채워지는지 본다.

- [ ] **Step 4: 기록하고 커밋한다**

```bash
git add spikes/slint-shellmenu docs/spikes/m0-report.md
git commit -m "docs(spike): record Slint gate 4 native shell context menu"
```

---

### Task 8: [Windows 실기 필요] [macOS 실기 필요] 게이트 6 스크린 리더(A11Y-02)

**Files:**
- Create: `spikes/slint-a11y/Cargo.toml`, `build.rs`, `ui/app.slint`, `src/main.rs`
- Modify: `docs/spikes/m0-report.md`(G6, 미달이면 보완 계획)

**Interfaces:**
- Consumes: 없음(목록 10만 행은 메모리에서 만든다).
- Produces: G6 행과 보완 계획. Slint 1.18의 `AccessibleRole`에는 `list`, `list-item`, `tree`, `button`, `checkbox`는 있고 `tree-item`·`dialog`는 없다(소스 확인). 이 차이를 G6-4·G6-5 메모에 적는다.

- [ ] **Step 1: harness를 만든다**

```bash
cargo new --vcs none --name rfspike-slint-a11y spikes/slint-a11y
cargo add --manifest-path spikes/slint-a11y/Cargo.toml slint@1.17
cargo add --manifest-path spikes/slint-a11y/Cargo.toml --build slint-build@1.17
```
화면(탭 순서대로):
1. 사용자 정의 목록: `ListView`(`accessible-role: list`) 안에 10만 행, 행마다 `accessible-role: list-item`, `accessible-label: "항목 {i}.txt"`, `accessible-item-index: i`, `accessible-item-count: 100000`, `accessible-item-selectable: true`, `accessible-item-selected: i == current`. `FocusScope`가 ↑↓로 `current`를 옮기고 보이게 스크롤한다.
2. 참고 목록: `StandardListView`(항목 1,000개).
3. 트리: `accessible-role: tree` 컨테이너, 노드 행은 `accessible-role: list-item`, `accessible-expandable: true`, `accessible-expanded: <상태>`, `accessible-action-expand`.
4. 버튼 "복사"와 체크박스 "숨김 파일 표시"(상태 확인용).
5. 버튼 "삭제…"가 여는 `Dialog`(제목 "삭제 확인", `StandardButton { kind: ok; }`, `StandardButton { kind: cancel; }`).

Run: `cargo build --release --manifest-path spikes/slint-a11y/Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 2: Windows에서 듣는다**

NVDA(설치)와 Accessibility Insights for Windows를 설치한다. `.\spikes\slint-a11y\target\release\rfspike-slint-a11y.exe`를 띄운다.
- G6-1: 내레이터(Win+Ctrl+Enter)를 켜고 사용자 정의 목록에 포커스 → ↓ 세 번. 들은 문장을 메모에 적는다.
- G6-2: NVDA로 같은 절차. NVDA 메뉴 > 도구 > 음성 뷰어의 텍스트를 메모에 붙인다.
- G6-4: 트리 펼침·접힘 상태, "복사" 버튼, 체크박스 체크 상태, 대화상자 제목과 버튼을 NVDA로 듣고, Accessibility Insights의 Inspect로 각 요소의 UIA `ControlType`, `Name`, 상태 속성을 메모에 적는다. 사용자 정의 목록이 UIA 트리에 노출하는 항목 수도 적는다(가상화 영향).

- [ ] **Step 3: macOS에서 듣는다**

`./spikes/slint-a11y/target/release/rfspike-slint-a11y`를 띄우고 VoiceOver(Cmd+F5)로 G6-3, G6-5를 같은 절차로 확인한다. VoiceOver 자막 패널의 문장을 메모에 적는다. Xcode가 설치돼 있으면 Accessibility Inspector로 역할·이름을 함께 적는다(없으면 메모에 "Accessibility Inspector 없음").

- [ ] **Step 4: 기록하고, 미달이면 보완 계획을 쓴다**

G6-1~G6-3 중 FAIL이 있으면 보고서 G6의 "보완 계획"에 행 ID마다 한 줄씩(문제 / OS 계층 보완 방법: 예 AccessKit 노드 직접 제공, UIA·NSAccessibility 공급자 / 반영 마일스톤 M1 또는 M5)을 쓴다.

```bash
git add spikes/slint-a11y docs/spikes/m0-report.md
git commit -m "docs(spike): record Slint gate 6 screen reader support on <windows|macos>"
```

---

### Task 9: [Windows 실기 필요] 탐색기 대체 런처와 WINX-02 진입 경로

**Files:**
- Create: `spikes/winx-launcher/Cargo.toml`, `src/lib.rs`, `src/cmdline.rs`, `src/pipe.rs`, `src/ledger.rs`
- Create: `spikes/winx-launcher/src/bin/{rfspike-open.rs, rfspike-main.rs, rfspike-probe.rs, rfspike-register.rs}`
- Modify: `docs/spikes/m0-report.md`(C1, C2)

**Interfaces:**
- Consumes: `fixtures/launcher`(Task 1).
- Produces:
  - `rfspike_winx::cmdline::launcher_target(raw: &str) -> Option<String>` — `GetCommandLineW` 원문에서 실행 파일 다음 첫 인자를 경로로 되살린다.
  - 파이프 프로토콜: 이름 `\\.\pipe\rfspike-open-<SessionId>`, 요청 한 줄 JSON `{"v":1,"cmd":"open","source":"<shell|wine|virtual|select>","path":"<p>","select":[...]}` + `\n`, 응답 `OK\n`.
  - `rfspike-main [--hang] [--log <path>]` 로그 줄 `OPEN source=<s> path="<p>" select=[...] t=<FILETIME>`(기본 `%LOCALAPPDATA%\rfspike\main.log`). Task 10·11이 옵션을 더한다.
  - `rfspike-open.exe` 로그 줄 `START raw=<...> target="<p>"`, `SENT elapsed_us=<n>`, `SPAWNED main elapsed_us=<n>`, `FALLBACK reason=<timeout|spawn-failed|virtual> method=<F1|F2> elapsed_ms=<n>`(`%LOCALAPPDATA%\rfspike\open.log`).
  - `rfspike-register apply --variant <v1|v2|v3> [--desktop-icons] | revert | status`, 원장 `%LOCALAPPDATA%\rfspike\winx-ledger.json`(키·값·종류·바꾸기 전 값·없었음 여부). Task 10이 `--wine-registry`, `--wine-disabledhotkeys`를 더한다.
  - `rfspike-probe shellexec [--verb open|explore] <path>`, `time-launcher --exe <p> --target <dir> --runs <n> [--cold-main]`, `paths --root <fixtures\launcher>`, `explorer-windows`.

- [ ] **Step 1: 의존성과 릴리스 프로필을 둔다**

```bash
cargo new --vcs none --lib --name rfspike-winx spikes/winx-launcher
cargo add --manifest-path spikes/winx-launcher/Cargo.toml serde -F derive
cargo add --manifest-path spikes/winx-launcher/Cargo.toml serde_json
cargo add --manifest-path spikes/winx-launcher/Cargo.toml windows --target 'cfg(target_os = "windows")' -F Win32_Foundation,Win32_Security,Win32_Storage_FileSystem,Win32_System_Com,Win32_System_IO,Win32_System_Pipes,Win32_System_Registry,Win32_System_RemoteDesktop,Win32_System_Threading,Win32_System_Variant,Win32_UI_Accessibility,Win32_UI_Input_KeyboardAndMouse,Win32_UI_Shell,Win32_UI_Shell_Common,Win32_UI_WindowsAndMessaging
```
`Cargo.toml`에 `[profile.release] opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`. `rfspike-open.rs` 맨 위에 `#![windows_subsystem = "windows"]`. 런처는 serde를 쓰지 않고 JSON 한 줄을 직접 만든다(크기).

- [ ] **Step 2: 인자 복원(RF1)의 실패하는 테스트를 쓴다**

`src/cmdline.rs`의 `#[cfg(test)]`(Linux에서도 돈다):

```rust
#[test]
fn drive_root_with_trailing_backslash_quote() {
    assert_eq!(launcher_target(r#""C:\rf\rfspike-open.exe" "C:\""#).as_deref(), Some(r"C:\"));
}

#[test]
fn spaces_hangul_emoji_percent_ampersand() {
    for p in [r"D:\rf spike 공백", r"D:\한글 폴더", r"D:\📁 emoji", r"D:\100% & co"] {
        let raw = format!(r#""C:\rf\rfspike-open.exe" "{p}""#);
        assert_eq!(launcher_target(&raw).as_deref(), Some(p));
    }
}

#[test]
fn unquoted_and_flags() {
    assert_eq!(launcher_target(r"C:\rf\rfspike-open.exe C:\Windows").as_deref(), Some(r"C:\Windows"));
    assert_eq!(launcher_target(r#""C:\rf\rfspike-open.exe" --wine"#), None);
}
```

Run: `cargo test --release --manifest-path spikes/winx-launcher/Cargo.toml --lib`
Expected: FAIL, `cannot find function `launcher_target``.

- [ ] **Step 3: `launcher_target`을 구현한다**

`pub fn launcher_target(raw: &str) -> Option<String>`: 실행 파일 토큰(따옴표면 닫는 따옴표까지)을 건너뛰고, 다음 토큰이 `--`로 시작하면 `None`. 따옴표로 시작하면 원문의 마지막 `"`까지를 그대로 취하고(백슬래시를 이스케이프로 보지 않음), 아니면 공백 전까지. 이렇게 하면 `"C:\"`가 `C:\`로 남는다.

Run: `cargo test --release --manifest-path spikes/winx-launcher/Cargo.toml --lib`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 4: 런처, 가짜 본체, 등록 도구, 탐침을 만든다**

- `rfspike-open`: `GetCommandLineW` → `launcher_target`. `::{`로 시작하거나 `shell:`이면 가상 위치로 보고 F2 폴백. 파이프에 연결해 요청을 보내고 `OK`를 3초 안에 받으면 종료. 파이프가 없으면 옆의 `rfspike-main.exe`를 띄우고 `WaitNamedPipeW`로 남은 시간까지 기다린다. 전체 3초 예산을 넘기거나 띄우기 실패면 폴백. 폴백 F1은 `C:\Windows\explorer.exe "<path>"` 직접 실행, F2는 `ShellExecuteExW`에 `SEE_MASK_CLASSKEY`와 `HKEY_LOCAL_MACHINE\SOFTWARE\Classes\Folder` 키, 동사 `open`(HKCU 재정의를 거치지 않음). 기본은 F2, 실행 인자 `--fallback F1`로 바꾼다.
- `rfspike-main`: 세션별 파이프 서버, 요청마다 로그 후 `OK`. `--hang`은 연결만 받고 응답하지 않는다.
- `rfspike-register`: 아래 변형을 원장에 바꾸기 전 값과 함께 기록하며 적용하고, `revert`는 역순으로 되돌리며, 둘 다 끝에 `SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None)`. 명령 값은 `"<rfspike-open.exe 전체 경로>" "%1"`.
  - V1(기본 동사 추가): `HKCU\Software\Classes\Directory\shell` 기본값 `rfspike`, `...\Directory\shell\rfspike\command` 기본값; `Drive\shell`도 같게.
  - V2(open·explore 재정의): `HKCU\Software\Classes\Directory\shell\open\command`와 `...\explore\command`의 기본값과 빈 `DelegateExecute`; `Drive`도 같게.
  - V3(Folder 재정의): `HKCU\Software\Classes\Folder\shell\open\command`와 `...\explore\command`의 기본값과 빈 `DelegateExecute`.
  - `--desktop-icons`: `HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}`(내 PC), `{645FF040-5081-101B-9F08-00AA002F954E}`(휴지통), `{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}`(네트워크) 각각의 `shell\open\command` 기본값 `"<exe>" --virtual <mycomputer|recyclebin|network>`와 빈 `DelegateExecute`.
- `rfspike-probe`: `shellexec`는 `ShellExecuteExW`(동사 없음·`open`·`explore`), `time-launcher`는 런처를 `--runs`번 띄워 시작부터 종료까지 ms를 재고 `RESULT C1-3 runs=<n> median_ms=<f> p95_ms=<f> verdict=<PASS if p95 < 20>`(`--cold-main`이면 회차마다 본체를 끄고 `RESULT C1-4 ... verdict=INFO`), `paths`는 `fixtures\launcher`의 폴더 5개와 `C:\`를 `shellexec`로 열고 `main.log`의 `path`가 각 원본과 같은지 비교해 `RESULT C1-7 ok=<k>/6 verdict=<PASS if 6/6>`, `explorer-windows`는 `IShellWindows::Count`를 출력.

Run: `cargo build --release --manifest-path spikes/winx-launcher/Cargo.toml; (Get-Item spikes\winx-launcher\target\release\rfspike-open.exe).Length`
Expected: `Finished `release` profile [optimized]`와 바이트 수(C1-1, 1,048,576 미만이면 PASS).

- [ ] **Step 5: ARM64 빌드 크기를 잰다**

Run: `rustup target add aarch64-pc-windows-msvc; cargo build --release --target aarch64-pc-windows-msvc --manifest-path spikes\winx-launcher\Cargo.toml --bin rfspike-open; (Get-Item spikes\winx-launcher\target\aarch64-pc-windows-msvc\release\rfspike-open.exe).Length`
Expected: 바이트 수(C1-2). 빌드가 실패하면 오류 첫 줄을 메모에 적고 판정 FAIL.

- [ ] **Step 6: 백업하고 V1을 적용해 C1을 잰다**

```powershell
reg export HKCU\Software\Classes "$env:USERPROFILE\rfspike-hkcu-classes.reg" /y
$W = ".\spikes\winx-launcher\target\release"
Start-Process "$W\rfspike-main.exe"
& "$W\rfspike-register.exe" apply --variant v1 --desktop-icons
& "$W\rfspike-probe.exe" time-launcher --exe "$W\rfspike-open.exe" --target "$env:RFSPIKE_DATA\thousand" --runs 50
& "$W\rfspike-probe.exe" time-launcher --exe "$W\rfspike-open.exe" --target "$env:RFSPIKE_DATA\thousand" --runs 10 --cold-main
```
Expected: `RESULT C1-3 runs=50 ...`, `RESULT C1-4 runs=10 ...`.

- [ ] **Step 7: 폴백을 시험한다(C1-5, C1-6, C1-8)**

본체를 끄고 `rfspike-main.exe --hang`을 띄운 뒤 `rfspike-probe shellexec "$env:RFSPIKE_DATA\thousand"`. 통과 조건(C1-5): 약 3초 뒤 탐색기 창이 그 폴더로 열리고 `open.log`에 `FALLBACK reason=timeout method=F2`. 본체를 끄고 `rfspike-main.exe`의 이름을 잠시 바꾼 뒤 같은 명령. 통과 조건(C1-6): 탐색기가 열리고 `FALLBACK reason=spawn-failed`. C1-8: V2·V3에서도 같은 두 시험을 F1·F2로 각각 돌려, 폴백이 다시 런처를 부르는지(`open.log`에 같은 경로의 `START`가 반복되는지) 본다. 반복이 없으면 그 조합은 통과. 시험 후 이름을 되돌린다.

- [ ] **Step 8: 특수 경로(RF1)와 진입 경로 표를 시험한다**

Run: `& "$W\rfspike-probe.exe" paths --root "$env:RFSPIKE_DATA\fixtures\launcher"`
Expected: `RESULT C1-7 ok=6/6 verdict=PASS`.

V1, V2, V3을 차례로 `apply`(사이에 `revert`)하며 C2를 시험한다. 각 시험에서 `main.log`에 `OPEN`이 생기면 ✔, 탐색기가 열리면 ✘:
- C2-1: 바탕화면의 폴더 아이콘 더블클릭, `rfspike-probe shellexec C:\`, `rfspike-probe shellexec "$env:USERPROFILE\Documents"`.
- C2-2: 바탕화면 아이콘 설정에서 내 PC·휴지통·네트워크를 켜고 각각 더블클릭(`--desktop-icons` 적용 상태).
- C2-3: `rfspike-probe shellexec --verb open <dir>`, `--verb explore <dir>`.
- C2-4: Win+R에 `C:\Windows` 입력.
- C2-5: `control.exe`, Win+R `shell:::{26EE0668-A00A-44D7-9371-BEB064C98683}`, 설정 > 블루투스 및 장치 > 프린터. 통과 조건은 반대로 탐색기·제어판이 정상으로 열리는 것.
- C2-6: `rfspike-register revert` 후 `rfspike-register status`가 `clean`을 출력하고, `reg export HKCU\Software\Classes after.reg /y`와 Step 6 백업의 내용이 같다(`fc.exe /u` 결과 "차이점이 없습니다" 또는 "no differences encountered").

- [ ] **Step 9: 기록하고 커밋한다**

C2 행의 측정값에 변형별 결과를 적는다. 끝나면 `rfspike-register revert`가 적용된 상태인지 `status`로 다시 확인한다.

```bash
git add spikes/winx-launcher docs/spikes/m0-report.md
git commit -m "docs(spike): record explorer replacement launcher and entry path coverage"
```

---

### Task 10: [Windows 실기 필요] Win+E 가로채기 방식 비교(WINX-07)

**Files:**
- Create: `spikes/winx-launcher/src/wine.rs`
- Modify: `spikes/winx-launcher/src/bin/{rfspike-main.rs, rfspike-register.rs, rfspike-probe.rs}`, `docs/spikes/m0-report.md`(C3)

**Interfaces:**
- Consumes: Task 9의 파이프·로그·원장·`rfspike-probe explorer-windows`.
- Produces: `rfspike-main --wine-hook | --wine-hotkey`, `rfspike-register apply --wine-registry | --wine-disabledhotkeys`(원장 경유), `rfspike-probe wine --count <n> --interval-ms <n>`, 로그 `OPEN source=wine method=<A|B|C>`.

- [ ] **Step 1: 세 방식을 구현한다**

- 방식 A(`--wine-registry`): `HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command` 기본값 `"<rfspike-open.exe>" --wine`과 빈 `DelegateExecute`. 런처는 `--wine`이면 `source=wine`으로 홈 폴더를 보낸다.
- 방식 B(`--wine-disabledhotkeys` + `rfspike-main --wine-hotkey`): `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced`의 `DisabledHotkeys`(REG_SZ)에 `E`를 넣고 `Stop-Process -Name explorer -Force`로 탐색기를 다시 띄운 뒤, 본체가 `RegisterHotKey(None, 1, MOD_WIN | MOD_NOREPEAT, 'E')`로 `WM_HOTKEY`를 받는다. 등록 실패 오류 코드를 로그에 남긴다.
- 방식 C(`rfspike-main --wine-hook`): 메시지 루프가 있는 스레드에서 `SetWindowsHookExW(WH_KEYBOARD_LL, ...)`. 왼쪽·오른쪽 Win이 눌린 상태의 `E` keydown을 삼키고, Win을 뗄 때 시작 메뉴가 열리지 않도록 그 전에 가상 키 `0xE8` 누름·뗌을 `SendInput`으로 넣은 뒤 홈 폴더를 연다.
- `rfspike-probe wine`: `SendInput`으로 LWin↓ E↓ E↑ LWin↑을 `--interval-ms` 간격으로 `--count`번 보내고, 끝나기 전후의 `explorer-windows` 수를 출력한다.

Run: `cargo build --release --manifest-path spikes\winx-launcher\Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 2: 방식마다 안정성 점검 a~f를 돌린다**

방식 하나를 켜고(나머지는 끄고 `rfspike-register revert`로 되돌린 상태에서) 다음을 한다. 각 항목의 통과 조건:
- a: `rfspike-probe wine --count 20 --interval-ms 1500` → `main.log`에 새 `OPEN source=wine` 20줄, 탐색기 창 수 변화 0.
- b: 실제 키보드로 Win+E 5회 → `OPEN` 5줄, 탐색기 창 0.
- c: `Stop-Process -Name explorer -Force` 뒤 10초 기다려 a 반복 → 같은 결과.
- d: 재부팅·로그인 뒤(방식 B·C는 본체를 시작 프로그램처럼 먼저 띄움) a 반복 → 같은 결과.
- e: a·b 동안 시작 메뉴가 한 번도 열리지 않는다(눈으로 확인).
- f: 방식을 끄고 `rfspike-register revert` 뒤 Win+E → 탐색기가 열린다.

a~f 결과를 `a✔ b✔ c✔ d✘ e✔ f✔`처럼 측정값에 적고, Win+E부터 `OPEN` 기록까지의 지연(로그 시각 차)을 메모에 적는다.

- [ ] **Step 3: 기록하고 커밋한다**

```bash
git add spikes/winx-launcher docs/spikes/m0-report.md
git commit -m "docs(spike): compare Win+E interception methods on Windows 11"
```

---

### Task 11: [Windows 실기 필요] `/select` 가로채기 가능성(WINX-02)

**Files:**
- Create: `spikes/winx-launcher/src/catch.rs`
- Modify: `spikes/winx-launcher/src/bin/{rfspike-main.rs, rfspike-probe.rs}`, `docs/spikes/m0-report.md`(C4)

**Interfaces:**
- Consumes: Task 9의 등록 변형 V1~V3과 로그.
- Produces: `rfspike-main --catch-explorer`, 로그 `CAUGHT folder="<p>" select=[...] flash_ms=<n>`; `rfspike-probe select <file>`(SHOpenFolderAndSelectItems), `rfspike-probe explorer-select <file>`(`explorer.exe /select,<file>` 실행).

- [ ] **Step 1: 창 가로채기(M2)를 구현한다**

`catch.rs`의 `pub fn run_catcher(on_caught: impl Fn(Caught) + 'static) -> windows::core::Result<()>`:
- `SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, None, Some(cb), 0, 0, WINEVENT_OUTOFCONTEXT)`, 콜백에서 `GetClassNameW`가 `CabinetWClass`인 최상위 창만 받는다.
- `IShellWindows`를 돌며 `IWebBrowserApp::HWND`가 같은 항목을 찾아 `IServiceProvider` → `IShellBrowser` → `QueryActiveShellView` → `IFolderView`로 폴더(`IPersistFolder2::GetCurFolder` → `SHGetPathFromIDListW`)와 선택(`Items(SVGIO_SELECTION)` → `IShellItemArray` → `SIGDN_FILESYSPATH`)을 얻는다. 선택이 비면 50ms 간격으로 1초까지 다시 읽는다.
- 얻으면 `IWebBrowser2::Quit()`으로 탐색기 창을 닫고, `EVENT_OBJECT_SHOW`부터 `Quit`까지를 `flash_ms`로 기록한다.
- 본체가 스스로 탐색기를 띄운 뒤 3초 안에 나타난 창은 잡지 않는다(PRD WINX-08 "탐색기로 열기" 예외).

Run: `cargo build --release --manifest-path spikes\winx-launcher\Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 2: 다섯 진입 경로를 두 방식으로 시험한다**

대상 파일: `$env:RFSPIKE_DATA\fixtures\reveal\보고서 한글.txt`(Chrome·Edge 시험은 실제로 내려받은 파일). 
- M1: V1, V2, V3을 차례로 적용하고 본체를 띄운 상태에서 C4-1~C4-5를 실행한다. `main.log`에 `OPEN`이 생기면 ✔.
- M2: 등록을 모두 되돌리고 `rfspike-main --catch-explorer`를 띄운 뒤 C4-1~C4-5를 각 5회 실행한다. 통과 조건: 매번 `CAUGHT`의 `folder`와 `select`가 대상과 같고, 탐색기 창이 남지 않는다. `flash_ms` 중앙값을 적는다.
- C4-1 `rfspike-probe explorer-select <file>`, C4-2 `rfspike-probe select <file>`, C4-3 Chrome 다운로드 목록의 "폴더에서 보기", C4-4 Edge 다운로드의 "폴더에 표시", C4-5 VS Code 탐색기 우클릭 "Reveal in File Explorer".

- [ ] **Step 3: 기록하고 커밋한다**

```bash
git add spikes/winx-launcher docs/spikes/m0-report.md
git commit -m "docs(spike): record /select interception feasibility"
```

---

### Task 12: [macOS 실기 필요] NSFileViewer 앱 호환성과 재부팅(MACX-01, R5)

**Files:**
- Create: `spikes/macx-fileviewer/Cargo.toml`, `src/main.rs`, `Info.plist`, `scripts/bundle.sh`, `scripts/set-viewer.sh`
- Modify: `docs/spikes/m0-report.md`(C12)

**Interfaces:**
- Consumes: `fixtures/reveal`(Task 1).
- Produces: `~/Applications/RFSpikeViewer.app`(번들 ID `io.github.rfspike.viewer`), 로그 `~/Library/Logs/rfspike-viewer.log`의 줄 `EVENT class=<4cc> id=<4cc> sender_pid=<n> items=[...]`과 `OPEN_URLS items=[...]`; `set-viewer.sh on|off|status`(바꾸기 전 값은 `~/rfspike/nsfileviewer.before`).

- [ ] **Step 1: 뷰어 앱을 만든다**

```bash
cargo new --vcs none --name rfspike-macx spikes/macx-fileviewer
cargo add --manifest-path spikes/macx-fileviewer/Cargo.toml objc2
cargo add --manifest-path spikes/macx-fileviewer/Cargo.toml objc2-foundation -F NSAppleEventDescriptor,NSAppleEventManager,NSArray,NSString,NSURL
cargo add --manifest-path spikes/macx-fileviewer/Cargo.toml objc2-app-kit -F NSApplication,NSResponder,NSRunningApplication
```
`main.rs`(macOS 전용): `NSApplication`과 `define_class!`로 만든 delegate. `applicationWillFinishLaunching:`에서 `NSAppleEventManager::sharedAppleEventManager()`의 `setEventHandler:andSelector:forEventClass:andEventID:`로 `handleEvent:withReply:`를 (`****`,`****`), (`misc`,`mvis`), (`aevt`,`odoc`), (`aevt`,`oapp`), (`aevt`,`rapp`), (`GURL`,`GURL`)에 건다. 핸들러는 `eventClass`, `eventID`, `keySenderPIDAttr` 속성, `keyDirectObject` 파라미터(목록이면 항목마다 `fileURLValue`)를 로그에 적고 앱을 앞으로 가져온다. `application:openURLs:`도 `OPEN_URLS`로 기록한다.
`Info.plist`: `CFBundleIdentifier` `io.github.rfspike.viewer`, `CFBundleName` `RFSpikeViewer`, `CFBundleExecutable` `rfspike-macx`, `CFBundlePackageType` `APPL`, `LSMinimumSystemVersion` `15.0`, `NSPrincipalClass` `NSApplication`.
`bundle.sh`: 릴리스 빌드 → `~/Applications/RFSpikeViewer.app/Contents/{Info.plist,MacOS/rfspike-macx}` 구성 → `codesign --force --sign - ~/Applications/RFSpikeViewer.app` → `/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f ~/Applications/RFSpikeViewer.app`.
`set-viewer.sh on`: 처음 한 번 `defaults read -g NSFileViewer`의 값(없으면 `ABSENT`)을 `~/rfspike/nsfileviewer.before`에 저장하고 `defaults write -g NSFileViewer -string io.github.rfspike.viewer`. `off`: 저장 값으로 되돌리거나 `ABSENT`면 `defaults delete -g NSFileViewer`. `status`: 현재 값 출력.

Run: `./spikes/macx-fileviewer/scripts/bundle.sh && codesign -dv ~/Applications/RFSpikeViewer.app 2>&1 | grep Identifier`
Expected: `Identifier=io.github.rfspike.viewer`.

- [ ] **Step 2: 앱마다 "Finder에서 보기"를 시험한다**

`./spikes/macx-fileviewer/scripts/set-viewer.sh on` 뒤, 각 앱을 새로 실행해 `$RFSPIKE_DATA/fixtures/reveal/보고서 한글.txt`(브라우저는 내려받은 파일)에 대해 보기 동작을 한다. 뷰어가 실행 중이 아닐 때 한 번, 실행 중일 때 한 번. 결과는 C12 판정 규칙대로, 로그에 찍힌 `class/id`를 측정값에 적는다.
- C12-1 `open -R "$RFSPIKE_DATA/fixtures/reveal/보고서 한글.txt"`, C12-2 Safari 다운로드 돋보기, C12-3 Chrome 다운로드 "Finder에서 보기", C12-4 Firefox 다운로드 폴더 아이콘, C12-5 Mail 첨부를 저장한 뒤 "Finder에서 보기", C12-6 VS Code "Reveal in Finder", C12-7 Xcode 파일 우클릭 "Show in Finder", C12-8 Spotlight 결과에서 Cmd+R, C12-9 미리보기 제목 막대 메뉴 "Finder에서 보기", C12-10 TextEdit 제목 막대 메뉴 "Finder에서 보기".
- C12-11: `set-viewer.sh off` 상태에서 Safari·Chrome·VS Code를 먼저 띄워 두고 `on`으로 바꾼 뒤(앱은 재시작하지 않음) 같은 동작.

- [ ] **Step 3: 재부팅 후와 되돌리기를 시험한다**

C12-12: `on` 상태로 재부팅 → 로그인 → C12-1, C12-2, C12-3, C12-6을 반복. C12-13: `set-viewer.sh off` 뒤 `set-viewer.sh status`가 원래 값(또는 `ABSENT`)을 출력하고 `open -R`이 Finder를 연다.

- [ ] **Step 4: 기록하고 커밋한다**

```bash
git add spikes/macx-fileviewer docs/spikes/m0-report.md
git commit -m "docs(spike): record NSFileViewer app compatibility and reboot behaviour"
```

---

### Task 13: 복사 엔진 harness와 정확성 검증(Linux에서 가능)

**Files:**
- Create: `spikes/copy-engine/Cargo.toml`, `src/{lib.rs, verdict.rs, aligned.rs, small.rs, large.rs, seqwrite.rs, baseline.rs, main.rs}`

**Interfaces:**
- Consumes: `small-files`, `large-file` 데이터셋 규칙과 `rfspike-data`(Task 1).
- Produces: CLI `rfspike-copy`
  - `small --src <dir> --dst <dir> --workers <n> --method <os|rw> [--runs 5] [--flush <cmd|none>]` → `RESULT C5-small method=<m> workers=<n> median_s=<f> min_s=<f> max_s=<f> files_ok=<bool> cache=<c> verdict=INFO`
  - `baseline --src <dir> --dst-parent <dir> [--runs 5] [--flush <cmd|none>]` → Windows `IFileOperation`, macOS Finder AppleScript, `RESULT C5-baseline tool=<explorer|finder> median_s=<f> ...`
  - `large --src <file> --dst <file> --mode <pipeline|std|copyfile-nobuf> --buf-mib <n> [--bufs 4] [--runs 5] [--flush ..] [--verify]` → `RESULT C5-large mode=<m> buf_mib=<n> median_mbps=<f> size_ok=<bool> hash_ok=<bool> verdict=INFO`
  - `seqwrite --dir <dir> --size <bytes> --buf-mib <n> [--runs 3]` → `RESULT C5-seqwrite buf_mib=<n> median_mbps=<f>`
  - `judge07 --ours-s <f> --os-s <f>` → `RESULT C5-07 ratio=<f> verdict=<PASS|FAIL>`, `judge08 --copy-mbps <f> --device-mbps <f>` → `RESULT C5-08 ratio=<f> verdict=<PASS|FAIL>`
  - Linux·기타 OS에서는 무버퍼 플래그 없이 버퍼 I/O로 동작하고 모든 줄에 `verdict=INFO reason=non-target-os`.

- [ ] **Step 1: 프로젝트를 만든다**

```bash
cargo new --vcs none --lib --name rfspike-copy spikes/copy-engine
cargo add --manifest-path spikes/copy-engine/Cargo.toml clap -F derive
cargo add --manifest-path spikes/copy-engine/Cargo.toml xxhash-rust -F xxh3
cargo add --manifest-path spikes/copy-engine/Cargo.toml crossbeam-channel libc
cargo add --manifest-path spikes/copy-engine/Cargo.toml windows --target 'cfg(target_os = "windows")' -F Win32_Foundation,Win32_Storage_FileSystem,Win32_System_Com,Win32_System_IO,Win32_UI_Shell,Win32_UI_Shell_Common
```

- [ ] **Step 2: 판정·정렬 로직의 실패하는 테스트를 쓴다**

`src/verdict.rs`·`src/aligned.rs`의 테스트:

```rust
#[test]
fn split_aligned_unaligned_tail() {
    assert_eq!(split_aligned(10_000_000_007, 4096), (9_999_998_976, 1_031));
    assert_eq!(split_aligned(8192, 4096), (8192, 0));
}

#[test]
fn perf07_half_or_less_passes() {
    assert_eq!(perf07_verdict(10.0, 20.0), "PASS");
    assert_eq!(perf07_verdict(10.1, 20.0), "FAIL");
}

#[test]
fn perf08_95_percent_passes() {
    assert_eq!(perf08_verdict(1900.0, 2000.0), "PASS");
    assert_eq!(perf08_verdict(1899.0, 2000.0), "FAIL");
}

#[test]
fn cold_run_with_failed_flush_is_invalid() {
    assert_eq!(run_verdict(false, true), Some("INVALID"));
    assert_eq!(run_verdict(true, false), Some("INVALID"));
    assert_eq!(run_verdict(true, true), None);
}
```

Run: `cargo test --release --manifest-path spikes/copy-engine/Cargo.toml --lib`
Expected: FAIL, `cannot find function `split_aligned``.

- [ ] **Step 3: 로직과 harness를 구현한다**

- `pub fn split_aligned(len: u64, align: u64) -> (u64, u64)`, `pub fn perf07_verdict(ours_s: f64, os_s: f64) -> &'static str`(ours ≤ 0.5 × os), `pub fn perf08_verdict(copy_mbps: f64, device_mbps: f64) -> &'static str`(copy ≥ 0.95 × device), `pub fn run_verdict(flush_ok: bool, release: bool) -> Option<&'static str>`.
- `small`: 대상 폴더 구조를 먼저 만들고(PRD XFER-04), crossbeam 채널 작업 큐와 워커 n개. `--method os`는 Windows `CopyFileExW`, macOS `std::fs::copy`(fcopyfile), `rw`는 256KiB 버퍼 읽기·쓰기. 회차마다 대상 삭제 → flush → 시간 측정. 끝나면 파일 수와 크기 합을 원본과 비교해 `files_ok`.
- `large --mode pipeline`: 읽기 스레드와 쓰기 스레드가 4096 정렬 버퍼 `--bufs`개를 채널로 돌린다. Windows는 원본·대상 모두 `FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN`, 대상은 `SetFileInformationByHandle(FileAllocationInfo)`로 미리 할당. 끝자락은 `split_aligned`의 나머지를 섹터 크기로 올려 쓰고 `SetFileInformationByHandle(FileEndOfFileInfo)`로 정확한 크기로 자른다. macOS는 `fcntl(F_NOCACHE, 1)`과 `F_PREALLOCATE`. `--mode std`는 `std::fs::copy`, `copyfile-nobuf`(Windows)는 `CopyFileExW(..., COPY_FILE_NO_BUFFERING)`. `--verify`는 측정 시간 밖에서 크기와 xxh3를 원본과 비교.
- `seqwrite`: `large`와 같은 무버퍼 열기로 같은 크기를 쓰기만 하고 파일을 지운다.
- `baseline`: Windows `CoCreateInstance(FileOperation)` → `SetOperationFlags(FOF_NO_UI)` → `SHCreateItemFromParsingName`(원본·대상 부모) → `CopyItem` → `PerformOperations`. macOS `osascript -e 'with timeout of 3600 seconds' -e 'tell application "Finder" to duplicate (POSIX file "<src>" as alias) to (POSIX file "<dst-parent>" as alias)' -e 'end timeout'`.

Run: `cargo test --release --manifest-path spikes/copy-engine/Cargo.toml --lib`
Expected: `test result: ok. 4 passed`.

- [ ] **Step 4: Linux에서 정확성을 확인한다**

```bash
D=/tmp/rfspike-copy && rm -rf $D && mkdir -p $D/src $D/dst
G=spikes/dataset-gen/target/release/rfspike-data; C=spikes/copy-engine/target/release/rfspike-copy
cargo build --release --manifest-path spikes/copy-engine/Cargo.toml
$G small-files --out $D/src --count 1000 && $G large-file --out $D/src --size 1000000007
$C small --src $D/src/small-files --dst $D/dst/small --workers 4 --method rw --runs 1 --flush none
$C large --src $D/src/large-file/large.bin --dst $D/dst/large.bin --mode pipeline --buf-mib 4 --runs 1 --flush none --verify
```
Expected: `RESULT C5-small ... files_ok=true ... verdict=INFO reason=non-target-os`와 `RESULT C5-large ... size_ok=true hash_ok=true verdict=INFO reason=non-target-os`.

- [ ] **Step 5: 커밋한다**

```bash
git add spikes/copy-engine
git commit -m "chore(spike): add copy engine prototype harness"
```

---

### Task 14: [Windows 실기 필요] [macOS 실기 필요] PERF-07·PERF-08 측정

**Files:**
- Modify: `docs/spikes/m0-report.md`(C5)

**Interfaces:**
- Consumes: Task 13의 `rfspike-copy`, `small-files`·`large-file`(Task 2·3), `RFSPIKE_DST`.
- Produces: C5 행. 가장 빠른 워커 수·버퍼 크기·모드를 메모에 남겨 M2 XFER 계획이 쓴다.

- [ ] **Step 1: Windows에서 작은 파일을 잰다(C5-1)**

```powershell
$C = ".\spikes\copy-engine\target\release\rfspike-copy.exe"; $F = "C:\tools\RAMMap64.exe -accepteula -Et"
cargo test --release --manifest-path spikes\copy-engine\Cargo.toml --lib; cargo build --release --manifest-path spikes\copy-engine\Cargo.toml
& $C baseline --src "$env:RFSPIKE_DATA\small-files" --dst-parent "$env:RFSPIKE_DST\base" --flush $F
foreach ($w in 1,4,8,16) { foreach ($m in "os","rw") { & $C small --src "$env:RFSPIKE_DATA\small-files" --dst "$env:RFSPIKE_DST\small" --workers $w --method $m --flush $F } }
```
Expected: `test result: ok. 4 passed`, `RESULT C5-baseline tool=explorer ...`, 조합 8개의 `RESULT C5-small ... files_ok=true cache=cold`. 가장 빠른 조합의 `median_s`로 `& $C judge07 --ours-s <값> --os-s <baseline 값>` → `RESULT C5-07 ... verdict=...`. Defender 실시간 보호는 켠 상태로 잰다(탐색기와 같은 조건).

- [ ] **Step 2: RF5를 확인한다**

Run: `& $C small --src "$env:RFSPIKE_DATA\small-files" --dst "$env:RFSPIKE_DST\small" --workers 4 --method os --runs 1 --flush "cmd /c exit 1"`
Expected: `verdict=INVALID reason=flush-failed`.

- [ ] **Step 3: Windows에서 10GB 파일을 잰다(C5-3, C5-5)**

DiskSpd(https://github.com/microsoft/diskspd/releases)를 `C:\tools\diskspd.exe`에 둔다.

```powershell
foreach ($b in 1,4,16,64) { & $C seqwrite --dir $env:RFSPIKE_DST --size 10000000007 --buf-mib $b }
C:\tools\diskspd.exe -c10G -b4M -d30 -o8 -t1 -w100 -Su -si "$env:RFSPIKE_DST\diskspd.dat"; Remove-Item "$env:RFSPIKE_DST\diskspd.dat"
foreach ($b in 1,4,16,64) { & $C large --src "$env:RFSPIKE_DATA\large-file\large.bin" --dst "$env:RFSPIKE_DST\large.bin" --mode pipeline --buf-mib $b --flush $F --verify }
& $C large --src "$env:RFSPIKE_DATA\large-file\large.bin" --dst "$env:RFSPIKE_DST\large.bin" --mode std --buf-mib 4 --flush $F --verify
& $C large --src "$env:RFSPIKE_DATA\large-file\large.bin" --dst "$env:RFSPIKE_DST\large.bin" --mode copyfile-nobuf --buf-mib 4 --flush $F --verify
```
Expected: `RESULT C5-seqwrite` 4줄, DiskSpd 출력의 `total:` 줄(MiB/s), `RESULT C5-large` 6줄 모두 `size_ok=true hash_ok=true`. 장치 대역폭 = seqwrite 최댓값과 DiskSpd MiB/s(MB/s로 환산) 중 큰 값. `& $C judge08 --copy-mbps <pipeline 최댓값> --device-mbps <장치 대역폭>` → `RESULT C5-08 ...`. C5-5는 모든 `large` 줄이 `size_ok=true hash_ok=true`면 PASS.

- [ ] **Step 4: Windows 결과를 기록하고 커밋한다**

```bash
git add docs/spikes/m0-report.md
git commit -m "docs(spike): record PERF-07 and PERF-08 on Windows"
```

- [ ] **Step 5: macOS에서 작은 파일을 잰다(C5-2)**

```zsh
C=./spikes/copy-engine/target/release/rfspike-copy; F="sudo -n purge"; sudo -v
cargo test --release --manifest-path spikes/copy-engine/Cargo.toml --lib && cargo build --release --manifest-path spikes/copy-engine/Cargo.toml
$C baseline --src "$RFSPIKE_DATA/small-files" --dst-parent "$RFSPIKE_DST/base" --flush "$F"
for w in 1 4 8 16; do for m in os rw; do $C small --src "$RFSPIKE_DATA/small-files" --dst "$RFSPIKE_DST/small" --workers $w --method $m --flush "$F"; done; done
```
Expected: `RESULT C5-baseline tool=finder ...`와 `RESULT C5-small` 8줄, 그다음 `judge07` 결과.

- [ ] **Step 6: macOS에서 10GB 파일을 잰다(C5-4, C5-5)**

```zsh
for b in 1 4 16 64; do $C seqwrite --dir "$RFSPIKE_DST" --size 10000000007 --buf-mib $b; done
for b in 1 4 16 64; do $C large --src "$RFSPIKE_DATA/large-file/large.bin" --dst "$RFSPIKE_DST/large.bin" --mode pipeline --buf-mib $b --flush "$F" --verify; done
$C large --src "$RFSPIKE_DATA/large-file/large.bin" --dst "$RFSPIKE_DST/large.bin" --mode std --buf-mib 4 --flush "$F" --verify
```
Expected: `RESULT C5-seqwrite` 4줄, `RESULT C5-large` 5줄 모두 `size_ok=true hash_ok=true`, 그다음 `judge08` 결과(장치 대역폭은 seqwrite 최댓값).

- [ ] **Step 7: macOS 결과를 기록하고 커밋한다**

C5-5 메모에 두 OS의 `hash_ok` 결과를 함께 적는다.

```bash
git add docs/spikes/m0-report.md
git commit -m "docs(spike): record PERF-07 and PERF-08 on macOS"
```

---

### Task 15: [Windows 실기 필요] NTFS MFT·USN 색인 속도(PERF-13, IDX-02, IDX-03)

**Files:**
- Create: `spikes/index-ntfs/Cargo.toml`, `src/{main.rs, paths.rs, mft.rs, enum_usn.rs, walk.rs, usn.rs}`
- Modify: `docs/spikes/m0-report.md`(C9)

**Interfaces:**
- Consumes: VHDX `R:\`의 million 데이터셋과 `R:\rfspike-manifest.json`(Task 2), `paths_xxh3` 정의(Task 1).
- Produces: `rfspike-ntfs mft --volume R: [--runs 5] [--flush <cmd>] [--dump-diff <file>]` → `RESULT C9 method=mft entries=<n> seconds=<f> peak_mb=<n> paths_xxh3=<h> manifest_match=<bool> verdict=<PASS if seconds ≤ 10 && match>`; `enum-usn --volume R:` → `RESULT C9 method=enum-usn ... verdict=INFO`; `walk --root R:\ --threads <n>` → `RESULT C9 method=walk ... verdict=<PASS if seconds ≤ 180 && match>`; `usn-churn --volume R: --n 10000` → `RESULT C9 method=usn ops=<n> records=<n> read_ms=<f> missing=<n> latency_ms=<f> verdict=INFO`.

- [ ] **Step 1: 의존성을 추가한다**

```bash
cargo new --vcs none --name rfspike-ntfs spikes/index-ntfs
cargo add --manifest-path spikes/index-ntfs/Cargo.toml clap -F derive
cargo add --manifest-path spikes/index-ntfs/Cargo.toml xxhash-rust -F xxh3
cargo add --manifest-path spikes/index-ntfs/Cargo.toml serde_json
cargo add --manifest-path spikes/index-ntfs/Cargo.toml mft ntfs crossbeam-channel --target 'cfg(target_os = "windows")'
cargo add --manifest-path spikes/index-ntfs/Cargo.toml windows --target 'cfg(target_os = "windows")' -F Win32_Foundation,Win32_Security,Win32_Storage_FileSystem,Win32_System_IO,Win32_System_Ioctl,Win32_System_ProcessStatus,Win32_System_Threading
```

- [ ] **Step 2: 네 방식을 구현한다**

- 공통: 관리자 권한이 아니면 `관리자 권한 필요`를 출력하고 종료 코드 2. 시간은 볼륨 열기부터 메모리 표(FRN → 부모 FRN, 이름, 크기, 수정·생성 시각, 속성) 완성까지. 경로 복원과 `paths_xxh3` 계산(`paths.rs`, Task 1 정의 복사)은 시간 밖에서 한다. `peak_mb`는 `GetProcessMemoryInfo`의 `PeakWorkingSetSize`.
- `mft`: `\\.\R:`를 `CreateFileW(GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, OPEN_EXISTING)`으로 열고, `ntfs` 크레이트(`Ntfs::new`, `ntfs.file(fs, 0)`, `$DATA`의 `data_runs()`)로 `$MFT`의 런을 얻는다(섹터 정렬 읽기 래퍼는 ntfs 크레이트 예제 `ntfs-shell`의 `SectorReader`와 같은 방식). 런을 16MiB 단위로 읽어 1KiB 레코드마다 `mft::MftEntry::from_buffer(buf, n)`로 파싱한다. `is_allocated()`가 아닌 레코드는 버리고, `$FILE_NAME`은 네임스페이스 DOS(2)를 버리며 남은 이름마다 항목 하나(하드 링크), 크기는 이름 없는 `$DATA`, 시각·속성은 `$STANDARD_INFORMATION`. 레코드 번호 24 미만과 루트 바로 아래 `$` 이름은 메타 파일로 제외.
- `enum-usn`: `FSCTL_QUERY_USN_JOURNAL`(없으면 `FSCTL_CREATE_USN_JOURNAL`) 뒤 `FSCTL_ENUM_USN_DATA`(`MFT_ENUM_DATA_V0 { StartFileReferenceNumber: 0, LowUsn: 0, HighUsn: NextUsn }`)를 1MiB 버퍼로 돌려 `USN_RECORD_V2`의 이름·부모를 모은다(레코드당 이름 하나라 하드 링크가 빠질 수 있음 — 메모에 적는다).
- `walk`: `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)` 재귀, 폴더 단위 작업 큐와 스레드 `--threads`개(IDX-03 대체 경로).
- `usn-churn`: `NextUsn`을 저장 → `R:\churn\`에 만들기 4,000, 이름 바꾸기 3,000, 지우기 3,000을 하며 기대 결과를 메모리에 남김 → 저장한 USN부터 `FSCTL_READ_USN_JOURNAL`로 읽어 집합에 적용 → 기대 결과와 비교해 `missing`. 이어서 파일 하나를 만든 시각부터 `READ_USN_JOURNAL`(`BytesToWaitFor = 1`, `Timeout`)이 그 레코드를 돌려줄 때까지를 `latency_ms`로 잰다.

Run: `cargo build --release --manifest-path spikes\index-ntfs\Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 3: 속도를 잰다(C9-1~C9-3)**

관리자 PowerShell:

```powershell
$N = ".\spikes\index-ntfs\target\release\rfspike-ntfs.exe"; $F = "C:\tools\RAMMap64.exe -accepteula -Et"
& $N mft --volume R: --runs 5 --flush $F
& $N enum-usn --volume R: --runs 5 --flush $F
foreach ($t in 1,8,16) { & $N walk --root R:\ --threads $t --runs 5 --flush $F }
```
Expected: `RESULT C9 method=mft ...`, `RESULT C9 method=enum-usn ...`, `RESULT C9 method=walk threads=<t> ...` 세 줄.

- [ ] **Step 4: 항목 집합을 확인한다(C9-4, RF4)**

Expected: `method=mft`와 `method=walk` 줄이 모두 `entries=1011101`, `manifest_match=true`이고 `paths_xxh3`가 `R:\rfspike-manifest.json`의 값과 같으면 PASS. 다르면 `& $N mft --volume R: --runs 1 --flush none --dump-diff "$env:RFSPIKE_DATA\ntfs-diff.txt"`를 실행한다. `--dump-diff`는 측정 뒤 같은 볼륨을 `walk` 방식으로 한 번 더 훑어, MFT 결과에만 있는 경로와 `walk` 결과에만 있는 경로를 앞에서 100개씩 파일에 쓴다(`walk` 자체가 매니페스트와 다르면 `rfspike-data verify --root R:\`로 먼저 확인). 원인(DOS 이름, 하드 링크, 미사용 레코드 등)을 메모에 적는다.

- [ ] **Step 5: USN 반영을 잰다(C9-5)**

Run: `& $N usn-churn --volume R: --n 10000`
Expected: `RESULT C9 method=usn ops=10000 ... missing=0 ...`. 끝나면 `R:\churn`을 지우고 `rfspike-data verify --root R:\`가 다시 `VERIFY ok`인지 확인한다(`churn`은 제외 대상이 아니므로 먼저 지운다).

- [ ] **Step 6: 기록하고 커밋한다**

```bash
git add spikes/index-ntfs docs/spikes/m0-report.md
git commit -m "docs(spike): record NTFS MFT, USN and folder walk indexing speed"
```

---

### Task 16: [macOS 실기 필요] macOS 디스크 훑기, `searchfs`, FSEvents 따라잡기(IDX-04, PERF-13)

**Files:**
- Create: `spikes/index-mac/Cargo.toml`, `src/{main.rs, paths.rs, bulk.rs, searchfs.rs, fsevents.rs, churn.rs}`
- Modify: `docs/spikes/m0-report.md`(C10)

**Interfaces:**
- Consumes: `/Volumes/RFSPIKE1M`과 매니페스트(Task 3), `paths_xxh3` 정의(Task 1).
- Produces: `rfspike-macidx bulk --root <p> --threads <n> [--skip-data-volume] [--dedupe] [--runs 5] [--flush <cmd>]` → `RESULT C10 method=bulk root=<p> entries=<n> seconds=<f> paths_xxh3=<h> manifest_match=<bool|n/a> dup_pairs=<n> eperm=<n> verdict=<v>`; `searchfs --volume <p>` → `RESULT C10 method=searchfs supported=<bool> files=<n> dirs=<n> seconds=<f> verdict=INFO`; `fsevents-id`; `churn --root <p> --n <n> --truth <file>`; `fsevents-catchup --root <p> --since <id> --truth <file>` → `RESULT C10 method=fsevents events=<n> history_done_ms=<f> missing=<n> extra=<n> verdict=<PASS if missing=0>`.

- [ ] **Step 1: 의존성을 추가한다**

```bash
cargo new --vcs none --name rfspike-macidx spikes/index-mac
cargo add --manifest-path spikes/index-mac/Cargo.toml clap -F derive
cargo add --manifest-path spikes/index-mac/Cargo.toml xxhash-rust -F xxh3
cargo add --manifest-path spikes/index-mac/Cargo.toml serde_json crossbeam-channel
cargo add --manifest-path spikes/index-mac/Cargo.toml libc fsevent-sys core-foundation --target 'cfg(target_os = "macos")'
```

- [ ] **Step 2: 네 기능을 구현한다**

- `bulk`: 폴더마다 `open(O_RDONLY | O_DIRECTORY)` 뒤 `libc::getattrlistbulk`를 큰 버퍼(256KiB)로 반복. 속성 `ATTR_CMN_RETURNED_ATTRS | ATTR_CMN_NAME | ATTR_CMN_DEVID | ATTR_CMN_OBJTYPE | ATTR_CMN_FILEID | ATTR_CMN_MODTIME | ATTR_CMN_CRTIME | ATTR_CMN_FLAGS`, `ATTR_FILE_DATALENGTH`. 폴더 단위 작업 큐와 스레드 `--threads`개. `--skip-data-volume`이면 `/System/Volumes/Data` 경로는 내려가지 않고, 장치 ID가 `/` 또는 `/System/Volumes/Data`의 장치 ID가 아닌 폴더(다른 볼륨)도 내려가지 않는다. `--dedupe`면 (장치 ID, 파일 ID) 쌍이 이미 있으면 `dup_pairs`를 센다. `EPERM`·`EACCES`는 `eperm`으로 센다. `--root`가 매니페스트를 가진 루트면 `paths_xxh3`를 비교해 `manifest_match`, PASS 조건은 seconds ≤ 180 && match.
- `searchfs`: `getattrlist(ATTR_VOL_CAPABILITIES)`의 `VOL_CAP_INT_SEARCHFS` 비트를 확인한다. 지원하면 `extern "C" fn searchfs`(구조체 `fssearchblock`, `searchstate`는 macOS SDK `sys/attr.h` 정의를 그대로 옮긴다, `man 2 searchfs`)로 두 번 검색한다: 파일은 `ATTR_FILE_DATALENGTH` 범위 [0, u64::MAX] + `SRCHFS_MATCHFILES`, 폴더는 `ATTR_DIR_ENTRYCOUNT` 범위 [0, u32::MAX] + `SRCHFS_MATCHDIRS`. 반환 속성은 `ATTR_CMN_NAME | ATTR_CMN_OBJID | ATTR_CMN_PAROBJID`. `EBUSY`(카탈로그 변경)면 처음부터 다시 하고 횟수를 메모에 남긴다.
- `fsevents-id`: `FSEventsGetCurrentEventId()` 출력.
- `churn`: 만들기 4,000, 이름 바꾸기 3,000, 지우기 3,000을 `--root/churn/` 아래에서 하고, 바뀐 경로 집합을 `--truth` 파일에 줄 단위로 쓴다.
- `fsevents-catchup`: `FSEventStreamCreate`(경로 `--root`, `sinceWhen = --since`, 플래그 `kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer`)를 런 루프에 붙이고, `kFSEventStreamEventFlagHistoryDone`이 올 때까지 받은 경로를 모아 `--truth`와 비교한다. `MustScanSubDirs`·`EventIdsWrapped`·`UserDropped`·`KernelDropped` 플래그가 오면 메모에 남긴다.

Run: `cargo build --release --manifest-path spikes/index-mac/Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 3: 100만 이미지와 전체 디스크를 훑는다(C10-1, C10-2, C10-6, C10-7)**

Terminal에 전체 디스크 접근 권한을 준다(시스템 설정 > 개인정보 보호 및 보안 > 전체 디스크 접근 권한).

```zsh
M=./spikes/index-mac/target/release/rfspike-macidx; sudo -v
for t in 1 8 16; do $M bulk --root /Volumes/RFSPIKE1M --threads $t --runs 5 --flush "sudo -n purge"; done
$M bulk --root / --threads 8 --dedupe --runs 1 --flush "sudo -n purge"
$M bulk --root / --threads 8 --dedupe --skip-data-volume --runs 1 --flush "sudo -n purge"
```
Expected: 100만 이미지 줄은 `entries=1011101 ... manifest_match=true`(C10-1·C10-7), 첫 번째 `/` 줄은 `dup_pairs`가 0보다 크고(펌링크 중복 확인), 두 번째 `/` 줄은 `dup_pairs=0`(C10-6 PASS). 두 `/` 줄의 `entries`, `seconds`, `eperm`을 C10-2에 적는다.

- [ ] **Step 4: `searchfs`를 확인한다(C10-3)**

Run: `$M searchfs --volume /Volumes/RFSPIKE1M; $M searchfs --volume /System/Volumes/Data`
Expected: `RESULT C10 method=searchfs supported=<...> ...` 두 줄. 판정: `supported=true`이고 1M 이미지에서 `files + dirs`가 1,010,101 이상(하드 링크 1,000개는 레코드 하나로 나올 수 있음)이며 `seconds`가 같은 이미지의 `bulk` 최소 `seconds`보다 작으면 "쓸 만함"(PASS), 아니면 FAIL과 이유.

- [ ] **Step 5: FSEvents 이력 따라잡기를 잰다(C10-4, C10-5)**

```zsh
ID=$($M fsevents-id); $M churn --root /Volumes/RFSPIKE1M --n 10000 --truth /tmp/rfspike-truth.txt
$M fsevents-catchup --root /Volumes/RFSPIKE1M --since $ID --truth /tmp/rfspike-truth.txt
```
Expected: `RESULT C10 method=fsevents ... missing=0 ...`(C10-4). 다시 `ID=$($M fsevents-id)`와 `churn`(새 truth 파일)을 한 뒤 재부팅하고, `hdiutil attach`로 이미지를 다시 붙여 같은 `--since $ID`로 `fsevents-catchup`을 돌린다(C10-5, ID는 재부팅 전에 파일로 저장해 둔다). 끝나면 `/Volumes/RFSPIKE1M/churn`을 지우고 `rfspike-data verify --root /Volumes/RFSPIKE1M`이 `VERIFY ok`인지 확인한다.

- [ ] **Step 6: 기록하고 커밋한다**

```bash
git add spikes/index-mac docs/spikes/m0-report.md
git commit -m "docs(spike): record macOS bulk scan, searchfs and FSEvents catch-up"
```

---

### Task 17: OpenDAL SFTP·OneDrive와 공급자 기능 부족(R9) (Linux에서 가능, Microsoft 계정 필요)

**Files:**
- Create: `spikes/opendal-remote/Cargo.toml`, `src/{main.rs, caps.rs, sftp_opendal.rs, sftp_russh.rs, onedrive.rs}`, `scripts/sftp-server.sh`
- Create: `spikes/opendal-remote/wincheck-openssh/{Cargo.toml, src/lib.rs}`, `spikes/opendal-remote/wincheck-russh/{Cargo.toml, src/lib.rs}`
- Modify: `docs/spikes/m0-report.md`(C6)

**Interfaces:**
- Consumes: `rfspike-data`(업로드용 파일), Docker.
- Produces: `rfspike-remote caps` → 공급자마다 `RESULT C6-caps service=<s> list=<b> write=<b> write_can_multi=<b> write_can_append=<b> rename=<b> copy=<b> create_dir=<b> delete=<b> stat=<b>`; `sftp-list|sftp-upload --impl <opendal|russh> --endpoint <ssh://user@host:port> --key <path> --root <p> [--file <local>]` → `RESULT C6-sftp impl=<i> op=<list|upload> entries=<n> bytes=<n> roundtrip_ok=<bool> verdict=<PASS|FAIL>`; `onedrive-list|onedrive-upload --token-file <p> --root <p> [--file <local>]` → `RESULT C6-onedrive op=<..> entries=<n> bytes=<n> roundtrip_ok=<bool> verdict=<..>`. 결과가 D11의 근거다.

- [ ] **Step 1: 의존성을 추가한다**

```bash
cargo new --vcs none --name rfspike-remote spikes/opendal-remote
cargo add --manifest-path spikes/opendal-remote/Cargo.toml opendal -F services-onedrive,services-ftp,services-webdav,services-gdrive,services-dropbox
cargo add --manifest-path spikes/opendal-remote/Cargo.toml opendal --target 'cfg(unix)' -F services-sftp
cargo add --manifest-path spikes/opendal-remote/Cargo.toml tokio -F full
cargo add --manifest-path spikes/opendal-remote/Cargo.toml russh russh-sftp clap -F clap/derive
cargo add --manifest-path spikes/opendal-remote/Cargo.toml xxhash-rust -F xxh3
```
`.secrets/`와 `.keys/`는 `spikes/.gitignore`가 막는다.

- [ ] **Step 2: harness를 만든다**

- `caps`: 각 공급자 빌더(`services::Ftp`, `Webdav`, `Gdrive`, `Onedrive`, `Dropbox`, unix에서 `Sftp`)를 더미 설정으로 `Operator::new(builder)?.finish()` 하고 `op.info().capability()`의 필드를 출력한다. 빌드가 네트워크를 요구해 실패하면 오류를 출력한다.
- `sftp-*` `--impl opendal`: `services::Sftp`의 `endpoint`, `user`, `key`, `root`, `known_hosts_strategy("accept")`. `--impl russh`: `russh::client::connect` → 키 인증 → `subsystem "sftp"` → `russh_sftp::client::SftpSession`의 `read_dir`·`create`·`write_all`. 업로드 후 다시 읽어 xxh3 비교가 `roundtrip_ok`.
- `onedrive-*`: `services::Onedrive`의 `access_token`(파일에서 읽음), `root`. 업로드는 `op.writer(path)`로 8MiB씩 써서 업로드 세션 경로를 타게 한다.
- `scripts/sftp-server.sh`: `ssh-keygen -t ed25519 -N "" -f spikes/opendal-remote/.keys/id_ed25519`(없을 때) → `docker run -d --name rfspike-sftp -p 2222:22 -v "$PWD/spikes/opendal-remote/.keys/id_ed25519.pub:/home/rfspike/.ssh/keys/id_ed25519.pub:ro" atmoz/sftp rfspike::1001::upload`.

Run: `cargo build --release --manifest-path spikes/opendal-remote/Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 3: SFTP를 시험한다(C6-1, C6-2, C6-12)**

```bash
./spikes/opendal-remote/scripts/sftp-server.sh && sleep 5
R=spikes/opendal-remote/target/release/rfspike-remote; G=spikes/dataset-gen/target/release/rfspike-data
$G large-file --out /tmp/rfspike-up --size 314572800 --name up300m.bin
for i in opendal russh; do
  $R sftp-upload --impl $i --endpoint ssh://rfspike@127.0.0.1:2222 --key spikes/opendal-remote/.keys/id_ed25519 --root /upload --file /tmp/rfspike-up/large-file/up300m.bin
  $R sftp-list --impl $i --endpoint ssh://rfspike@127.0.0.1:2222 --key spikes/opendal-remote/.keys/id_ed25519 --root /upload
done
```
Expected: 구현마다 `RESULT C6-sftp impl=<i> op=upload ... roundtrip_ok=true verdict=PASS`와 `op=list entries=` 1 이상 줄. Docker를 쓸 수 없으면 macOS에서 원격 로그인(sshd)을 켜고 `--endpoint ssh://<사용자>@127.0.0.1:22 --key ~/.ssh/id_ed25519 --root /tmp`로 같은 단계를 돈다.

- [ ] **Step 4: SFTP의 Windows 빌드 가능성을 확인한다(C6-3)**

`wincheck-openssh`는 `openssh`만, `wincheck-russh`는 `russh`(`--no-default-features -F ring`)와 `russh-sftp`만 의존하는 빈 lib 크레이트다.

```bash
cargo new --vcs none --lib --name rfspike-wincheck-openssh spikes/opendal-remote/wincheck-openssh
cargo add --manifest-path spikes/opendal-remote/wincheck-openssh/Cargo.toml openssh
cargo new --vcs none --lib --name rfspike-wincheck-russh spikes/opendal-remote/wincheck-russh
cargo add --manifest-path spikes/opendal-remote/wincheck-russh/Cargo.toml russh --no-default-features -F ring
cargo add --manifest-path spikes/opendal-remote/wincheck-russh/Cargo.toml russh-sftp
sudo apt-get install -y gcc-mingw-w64-x86-64 && rustup target add x86_64-pc-windows-gnu
cargo check --target x86_64-pc-windows-gnu --manifest-path spikes/opendal-remote/wincheck-openssh/Cargo.toml
cargo check --target x86_64-pc-windows-gnu --manifest-path spikes/opendal-remote/wincheck-russh/Cargo.toml
```
Expected: 첫 `cargo check`는 실패하고 오류에 `This crate can only be used on unix`(OpenDAL `services-sftp`가 의존하는 `openssh` 0.11의 `compile_error!`), 두 번째는 `Finished`. C6-3은 OpenDAL SFTP FAIL, 메모에 russh 결과.

- [ ] **Step 5: OneDrive를 시험한다(C6-4, C6-5)**

개인 Microsoft 계정으로 Graph Explorer(https://developer.microsoft.com/graph/graph-explorer)에 로그인해 `Files.ReadWrite`에 동의하고 "Access token"을 `spikes/opendal-remote/.secrets/onedrive.token`에 저장한다(1시간 유효).

```bash
T=spikes/opendal-remote/.secrets/onedrive.token; $G large-file --out /tmp/rfspike-up --size 16384 --name up16k.bin
$R onedrive-upload --token-file $T --root /rfspike --file /tmp/rfspike-up/large-file/up16k.bin
$R onedrive-upload --token-file $T --root /rfspike --file /tmp/rfspike-up/large-file/up300m.bin
$R onedrive-list --token-file $T --root /rfspike
```
Expected: `RESULT C6-onedrive op=upload ... roundtrip_ok=true verdict=PASS` 두 줄과 `op=list entries=2`. 컨테이너에서 `graph.microsoft.com`에 닿지 않으면 같은 명령을 macOS에서 돌리고 환경 열에 E-M1을 적는다.

- [ ] **Step 6: 공급자별 요구를 확인한다(C6-6~C6-11)**

Run: `$R caps`와 아래 소스 확인(경로는 `~/.cargo/registry/src/*/`):

```bash
S=$(ls -d ~/.cargo/registry/src/*/ | head -1)
grep -n "pub [a-z_]*:" $S/opendal-service-{ftp,sftp,webdav,gdrive,onedrive,dropbox}-*/src/config.rs
grep -rln "createUploadSession\|upload_session\|resumable" $S/opendal-service-{onedrive,dropbox,gdrive}-*/src
grep -rn "supportsAllDrives\|application/vnd.google-apps" $S/opendal-service-gdrive-*/src | head
grep -rn "resume\|offset" $S/opendal-core-*/src/types/write/*.rs | head
```
Expected: `RESULT C6-caps` 줄들과 grep 결과. 행마다 PRD 요구(기준 열)와 대조해 빠진 기능을 메모에 적는다. 확인 기준: FTP 설정에 능동·수동 모드와 파일명 인코딩 필드가 있는가, FTPS 명시적·암시적이 되는가; SFTP 설정에 비밀번호 인증과 지문 확인 콜백이 있는가; WebDAV에 다이제스트 인증이 있는가; Google Drive가 공유 드라이브(`supportsAllDrives`)와 Google 문서 MIME을 다루는가; 클라우드 3종에 업로드 세션이 있고, 끊긴 업로드를 이어 올리는 공개 API(재개 토큰·오프셋)가 있는가.

- [ ] **Step 7: 정리하고, 기록하고, 커밋한다**

`docker rm -f rfspike-sftp`, OneDrive의 `/rfspike` 폴더를 지운다.

```bash
git add spikes/opendal-remote docs/spikes/m0-report.md
git commit -m "docs(spike): record OpenDAL SFTP and OneDrive checks and provider gaps"
```

---

### Task 18: ZIP(AES)·7z 쓰기 라이브러리 비교(Linux에서 가능)

**Files:**
- Create: `spikes/archive-write/Cargo.toml`, `src/{main.rs, zipw.rs, sevenzw.rs, split.rs, inspect.rs}`, `scripts/bsdtar-probe.sh`
- Modify: `docs/spikes/m0-report.md`(C11)

**Interfaces:**
- Consumes: `rfspike-data`로 만든 입력(작은 파일 1만 개, 4,500,000,000바이트 파일, 1,000,000,007바이트 파일).
- Produces: `rfspike-arc zip --src <p> --out <zip> --password <pw> --level <n>`, `7z --src <p> --out <7z> --password <pw> [--no-header-encryption]`, `split --file <p> --size <bytes>`(→ `<p>.001`…), `zip-append --zip <p> --add <file>`, `zip-rename --zip <p> --from <name> --to <name> --out <p>`, `zip-delete --zip <p> --name <name> --out <p>`, `inspect --zip <p>` → `INSPECT entries=<n> utf8_flag=<all|some|none> zip64=<bool> aes=<all|some|none>`. 모든 명령은 `RESULT C11 lib=<zip|sevenz-rust2> op=<..> seconds=<f> bytes=<n>`. 결과가 D6·D7의 근거다.

- [ ] **Step 1: 도구와 의존성을 준비한다**

```bash
sudo apt-get install -y 7zip libarchive-tools time && 7z i | head -3 && bsdtar --version
cargo new --vcs none --name rfspike-arc spikes/archive-write
cargo add --manifest-path spikes/archive-write/Cargo.toml zip sevenz-rust2 walkdir clap -F clap/derive
```
Expected: `7-Zip` 판 정보 줄과 `bsdtar 3.` 줄.

- [ ] **Step 2: harness를 만든다**

- 항목 이름: `zip`·`7z` 모두 `--src` 기준 상대 경로를 `/` 구분자로 쓴다(예: `d000/s00000.bin`).
- `zip`: zip 크레이트 `ZipWriter`, `SimpleFileOptions::default().compression_method(Deflated).compression_level(Some(level)).large_file(true).with_aes_encryption(AesMode::Aes256, pw)`. 파일 이름은 UTF-8 문자열 그대로(비 ASCII 이름은 크레이트가 UTF-8 플래그를 세우는지 `inspect`로 확인).
- `7z`: sevenz-rust2 `ArchiveWriter::create(out)` → `set_content_methods(vec![AesEncoderOptions::new(pw).into(), LZMA2 옵션])` → 기본 `set_encrypt_header(true)`(`--no-header-encryption`이면 false) → 폴더의 항목 추가 → `finish`.
- `split`: 파일을 `--size` 바이트씩 `<p>.001`, `.002`…로 자른다(7z 다중 볼륨 형식).
- `zip-append`: `ZipWriter::new_append`. `zip-rename`·`zip-delete`: 원본을 읽으며 `raw_copy_file_rename`·건너뛰기로 새 파일을 쓴다(재압축 없음).
- `inspect`: 각 로컬 헤더의 일반 목적 비트 11(UTF-8), ZIP64 추가 필드(0x0001), AES 추가 필드(0x9901)를 직접 읽는다.
- `bsdtar-probe.sh`: `bsdtar --format zip --options zip:encryption=aes256 --passphrase rfspike -cf $OUT/bsdtar-aes.zip -C <src> .`와 `bsdtar --format 7zip --options 7zip:compression=lzma2 --passphrase rfspike -cf $OUT/bsdtar.7z -C <src> .`의 종료 코드와 stderr를 출력.

Run: `cargo build --release --manifest-path spikes/archive-write/Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 3: 측정하고 7-Zip으로 검증한다**

```bash
A=spikes/archive-write/target/release/rfspike-arc; G=spikes/dataset-gen/target/release/rfspike-data; O=/tmp/rfspike-arc; mkdir -p $O
$G small-files --out $O/in && $G fixtures --out $O/in && $G large-file --out $O/in4g --size 4500000000 && $G large-file --out $O/in1g --size 1000000007
/usr/bin/time -v $A zip --src $O/in/small-files --out $O/small.zip --password rfspike --level 6 2>&1 | grep -E "RESULT|Maximum resident"
$A zip --src $O/in/fixtures --out $O/names.zip --password rfspike --level 6 && $A inspect --zip $O/names.zip
$A zip --src $O/in4g/large-file --out $O/big.zip --password rfspike --level 1 && $A inspect --zip $O/big.zip
/usr/bin/time -v $A 7z --src $O/in/small-files --out $O/small.7z --password rfspike 2>&1 | grep -E "RESULT|Maximum resident"
$A 7z --src $O/in1g/large-file --out $O/big.7z --password rfspike && $A split --file $O/big.7z --size 104857600
7z t -prfspike $O/small.zip; 7z t -prfspike $O/big.zip; 7z t -prfspike $O/small.7z; 7z t -prfspike $O/big.7z.001
7z l $O/small.7z; 7z l -prfspike $O/names.zip
./spikes/archive-write/scripts/bsdtar-probe.sh $O/in/small-files $O
```
Expected: `7z t` 네 번 모두 `Everything is Ok`; 암호 없는 `7z l $O/small.7z`는 `Can not open encrypted archive. Wrong password?`(파일명 암호화 확인); `7z l -prfspike $O/names.zip`에 `보고서 한글.txt`가 깨지지 않고 보임; `INSPECT ... utf8_flag=` 값과 `big.zip`의 `zip64=true aes=all`. bsdtar 결과는 종료 코드와 메시지로 기록하고, 만들어진 파일이 있으면 `7z t -prfspike`로 검증한다.

- [ ] **Step 4: 기존 압축 파일 수정을 확인한다(C11-7)**

```bash
cp $O/small.zip $O/edit.zip && $A zip-append --zip $O/edit.zip --add "$O/in/fixtures/reveal/보고서 한글.txt"
$A zip-rename --zip $O/edit.zip --from "d000/s00000.bin" --to "d000/renamed.bin" --out $O/edit2.zip
$A zip-delete --zip $O/edit2.zip --name "d000/s00001.bin" --out $O/edit3.zip && 7z t -prfspike $O/edit3.zip
```
Expected: 각 명령의 `RESULT C11 ... op=<append|rename|delete> seconds=` 줄과 `Everything is Ok`. 7z 수정은 sevenz-rust2의 공개 API(`push_raw_block` 등)로 블록을 재압축 없이 옮길 수 있는지 문서·소스로 확인해 메모에 적는다(불가면 "전체 다시 쓰기").

- [ ] **Step 5: 기록하고 커밋한다**

C11 행마다 후보별 결과, 메모에 라이선스(zip MIT, sevenz-rust2 Apache-2.0, libarchive BSD)와 처리 시간·최대 메모리를 적는다. ZIP 분할(`.z01` 형식)은 zip 크레이트가 지원하지 않으면 C11-6 메모에 적는다.

```bash
git add spikes/archive-write docs/spikes/m0-report.md
git commit -m "docs(spike): compare ZIP AES and 7z write libraries"
```

---

### Task 19: [Windows 실기 필요] 변경 원장과 Windows VM 스냅샷 파이프라인, Inno Setup 연동(INST-02, INST-07, Q7)

**Files:**
- Create: `spikes/ledger-vm/Cargo.toml`, `src/{lib.rs, snapshot.rs, diff.rs, win_ops.rs, mac_ops.rs}`, `src/bin/{rfspike-ledger.rs, rfspike-snap.rs}`, `tests/diff.rs`
- Create: `spikes/ledger-vm/allowlist-windows.txt`, `spikes/ledger-vm/installer/rfspike.iss`, `spikes/ledger-vm/scripts/{win-vm-pipeline.ps1, win-guest-run.ps1}`
- Modify: `docs/spikes/m0-report.md`(C7-1, C7-2, C7-5, C7-6, C8)

**Interfaces:**
- Consumes: Task 9의 V1 등록 키 목록(시나리오에 포함).
- Produces:
  - `rfspike_ledger::snapshot::Entry` — `enum Entry { Reg { path: String, name: String, ty: String, data_xxh3: String }, File { path: String, size: u64, mtime: i64 }, Cmd { cmd: String, line: String } }`, `impl Entry { pub fn key(&self) -> String }`(Reg는 소문자로 비교), serde JSON Lines.
  - `rfspike_ledger::diff` — `pub struct Allowlist`, `pub fn parse_allowlist(text: &str) -> Result<Allowlist, String>`(줄 형식 `<kind>:<glob>\t<사유>`, 사유 없으면 Err, `%VAR%`·`~` 확장, `\`는 `/`로, 대소문자 무시), `pub enum Change { Added(Entry), Removed(Entry), Changed { before: Entry, after: Entry } }`, `pub struct DiffResult { pub unexpected: Vec<Change>, pub allowed: Vec<(Change, String)> }`, `pub fn diff(before: &[Entry], after: &[Entry], allow: &Allowlist) -> DiffResult`.
  - CLI `rfspike-snap capture --out <file> [--reg <hive\path>]... [--dir <path>]... [--cmd "<command>"]...`, `rfspike-snap diff --before <f> --after <f> --allow <f> --id <C7-1|C7-2|C7-3|C7-4>` → 차이 줄과 `RESULT <id> unexpected=<n> allowed=<n> verdict=<v>`(C7-1·C7-3은 unexpected=0이면 PASS, C7-2·C7-4는 unexpected=1이고 그 키에 `rfspikeleak`이 들어 있으면 PASS).
  - CLI `rfspike-ledger apply <base|service|all> [--leak] | revert | revert-service | uninstall-sequence | simulate-use | backup --to <zip> | status`, 원장 `%LOCALAPPDATA%\RFSpike\ledger.json`(macOS `~/Library/Application Support/io.github.rfspike.ledger/ledger.json`). Task 20이 macOS 연산을 더한다.

- [ ] **Step 1: 프로젝트를 만든다(Linux 가능)**

```bash
cargo new --vcs none --lib --name rfspike-ledger spikes/ledger-vm
cargo add --manifest-path spikes/ledger-vm/Cargo.toml serde -F derive
cargo add --manifest-path spikes/ledger-vm/Cargo.toml serde_json globset walkdir clap -F clap/derive
cargo add --manifest-path spikes/ledger-vm/Cargo.toml xxhash-rust -F xxh3
cargo add --manifest-path spikes/ledger-vm/Cargo.toml winreg --target 'cfg(target_os = "windows")'
cargo add --manifest-path spikes/ledger-vm/Cargo.toml windows --target 'cfg(target_os = "windows")' -F Win32_Foundation,Win32_Security,Win32_Security_Credentials,Win32_System_Services,Win32_System_Threading,Win32_UI_Shell
```

- [ ] **Step 2: 비교 로직(RF3)의 실패하는 테스트를 쓴다(Linux 가능)**

`spikes/ledger-vm/tests/diff.rs`:

```rust
use rfspike_ledger::{diff::*, snapshot::Entry};

fn reg(p: &str, d: &str) -> Entry { Entry::Reg { path: p.into(), name: "v".into(), ty: "REG_SZ".into(), data_xxh3: d.into() } }
fn file(p: &str, s: u64) -> Entry { Entry::File { path: p.into(), size: s, mtime: 0 } }

#[test]
fn added_removed_changed_are_reported() {
    let none = parse_allowlist("").unwrap();
    let r = diff(&[reg(r"HKCU\Software\A", "1"), file("C:/x", 10)], &[reg(r"HKCU\Software\A", "2"), file("C:/y", 5)], &none);
    assert_eq!(r.unexpected.len(), 3);
    assert!(r.allowed.is_empty());
}

#[test]
fn allowlist_suppresses_only_matching_entries() {
    let a = parse_allowlist("file:C:/Users/*/NTUSER.DAT*\t레지스트리 하이브 파일(내용은 reg 항목으로 비교)\n").unwrap();
    let r = diff(&[], &[file("C:/Users/t/NTUSER.DAT", 1), file("C:/Users/t/AppData/Local/RFSpike/x", 1)], &a);
    assert_eq!((r.unexpected.len(), r.allowed.len()), (1, 1));
}

#[test]
fn allowlist_line_without_reason_is_rejected() {
    assert!(parse_allowlist("file:C:/Windows/Prefetch/*\n").is_err());
}

#[test]
fn leak_is_exactly_one_unexpected_change() {
    let none = parse_allowlist("").unwrap();
    let r = diff(&[], &[reg(r"HKCU\Software\RFSpikeLeak", "1")], &none);
    assert_eq!(r.unexpected.len(), 1);
    assert!(matches!(&r.unexpected[0], Change::Added(e) if e.key().contains("rfspikeleak")));
}

#[test]
fn reg_keys_compare_case_insensitively() {
    let none = parse_allowlist("").unwrap();
    let r = diff(&[reg(r"HKCU\Software\A", "1")], &[reg(r"hkcu\software\a", "1")], &none);
    assert!(r.unexpected.is_empty());
}
```

Run: `cargo test --release --manifest-path spikes/ledger-vm/Cargo.toml --test diff`
Expected: FAIL, `unresolved import `rfspike_ledger::diff``.

- [ ] **Step 3: 비교 로직을 구현한다(Linux 가능)**

Interfaces의 시그니처대로 `snapshot.rs`와 `diff.rs`를 쓴다. 허용 목록 매칭 대상 문자열은 `<kind>:<path>`(Reg는 `reg:<path>\<name>`, Cmd는 `cmd:<cmd>:<line>`).

Run: `cargo test --release --manifest-path spikes/ledger-vm/Cargo.toml --test diff`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 4: Windows 원장 연산과 스냅샷 수집을 만든다**

`win_ops.rs`의 연산(각각 바꾸기 전 값을 원장에 쓰고 적용, `revert`는 역순 복원):
1. `HKCU\Software\RFSpike\Settings` 값 `Theme`=`dark`.
2. 파일 연결: `HKCU\Software\Classes\.rfspiketest` 기본값 `RFSpike.Test`, `HKCU\Software\Classes\RFSpike.Test\shell\open\command`.
3. 시작 프로그램: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 값 `RFSpike`.
4. 알림 등록: `HKCU\Software\Classes\AppUserModelId\RFSpike.Spike` 값 `DisplayName`.
5. 자격 증명: `CredWriteW`(대상 `rfspike:test`, `CRED_TYPE_GENERIC`).
6. 탐색기 대체 V1(Task 9 Step 4의 키 그대로).
7. `service`(관리자): `CreateServiceW("rfspike-svc", 실행 파일 = 설치 폴더의 rfspike-ledger.exe, SERVICE_DEMAND_START)`. 일반 권한 프로세스에서 호출되면 자신을 `ShellExecuteExW` 동사 `runas`로 `apply service`를 실행하고 기다린다.
`--leak`은 원장을 거치지 않고 `HKCU\Software\RFSpikeLeak` 값 `Leak`=1을 쓴다. `simulate-use`는 `%APPDATA%\RFSpike\settings.toml`, `%LOCALAPPDATA%\RFSpike\cache\thumb.bin`, `%LOCALAPPDATA%\RFSpike\logs\app.log`를 만들고, `%TEMP%\rfspike-<pid>.tmp`를 만들었다 지운다. `uninstall-sequence`는 PRD INST-04 순서대로 (1) `rfspike-main`·`rfspike-ledger` 다른 인스턴스 종료, (2) 원장 역순 복원(서비스가 있으면 `runas`로 `revert-service`), (3) 자격 증명 삭제, (4) `%APPDATA%\RFSpike`, `%LOCALAPPDATA%\RFSpike` 삭제를 하고 단계마다 `%TEMP%\rfspike-uninstall.log`에 시각과 함께 적은 뒤 그 로그도 지운다(지우기 전 내용을 stdout에 출력). `backup --to`는 `%APPDATA%\RFSpike`를 ZIP 하나로 묶는다(zip 의존성은 이 명령에만 쓰도록 `cargo add --manifest-path spikes/ledger-vm/Cargo.toml zip`).
`rfspike-snap capture`(Windows): `winreg`로 `--reg` 하이브를 재귀(접근 거부 키는 `denied=` 수로만 기록), `walkdir`로 `--dir`(크기·수정 시각), `--cmd` 출력 줄마다 `Cmd` 항목.

Run: `cargo build --release --manifest-path spikes\ledger-vm\Cargo.toml`
Expected: `Finished `release` profile [optimized]`.

- [ ] **Step 5: Inno Setup 스크립트를 쓴다**

`winget install --id JRSoftware.InnoSetup -e`. `installer/rfspike.iss`의 결정 사항:
- `[Setup]`: `AppId={{<GUID>}`(`<GUID>`는 `[guid]::NewGuid()`로 한 번 만든 값), `AppName=RFSpike`, `PrivilegesRequired=lowest`, `PrivilegesRequiredOverridesAllowed=dialog commandline`, `DefaultDirName={autopf}\RFSpike`(사용자 모드에서 `%LOCALAPPDATA%\Programs\RFSpike`), `ArchitecturesAllowed=x64compatible or arm64`, `ArchitecturesInstallIn64BitMode=x64compatible or arm64`, `OutputBaseFilename=rfspike-setup`, `UninstallDisplayName=RFSpike`.
- `[Tasks]`: `explorerreplace`(기본 체크), `indexservice`(기본 해제).
- `[Files]`: `rfspike-ledger.exe`, `rfspike-snap.exe`.
- `[Run]`: `Filename: "{app}\rfspike-ledger.exe"; Parameters: "apply base"; Flags: runhidden waituntilterminated` 와 `Tasks: indexservice`인 `Parameters: "apply service"; Verb: "runas"; Flags: shellexec waituntilterminated`.
- `[UninstallRun]`: `Filename: "{app}\rfspike-ledger.exe"; Parameters: "uninstall-sequence"; Flags: runhidden waituntilterminated; RunOnceId: "RFSpikeUninstall"`.
- `[Code]`: `InitializeUninstall`에서 `UninstallSilent`가 아니면 `CreateCustomForm`으로 체크박스 "삭제 전에 설정 백업"(기본 해제)을 띄우고, 체크하면 `Exec`로 `rfspike-ledger.exe backup --to "<문서 폴더>\RFSpike-backup.zip"`을 먼저 실행한다.

Run: `$iscc = (Get-ChildItem "C:\Program Files (x86)\Inno Setup *\ISCC.exe","$env:LOCALAPPDATA\Programs\Inno Setup *\ISCC.exe" -ErrorAction SilentlyContinue | Select-Object -First 1).FullName; & $iscc spikes\ledger-vm\installer\rfspike.iss`
Expected: `Successful compile` 과 `spikes\ledger-vm\installer\Output\rfspike-setup.exe`.

- [ ] **Step 6: Hyper-V VM을 준비한다**

Windows 11 Pro 이상 호스트에서 `Enable-WindowsOptionalFeature -Online -FeatureName Microsoft-Hyper-V -All`(재부팅). Windows 11 ISO로 VM `rfspike-win11`을 만들고, 관리자 계정 `rfadmin`과 표준 사용자 `rfspike`를 둔다. 업데이트 후 `Enable-VMIntegrationService -VMName rfspike-win11 -Name "Guest Service Interface"`, VM을 끄고 `Checkpoint-VM -Name rfspike-win11 -SnapshotName clean`.

- [ ] **Step 7: 파이프라인 스크립트를 쓴다**

`win-vm-pipeline.ps1 -VMName rfspike-win11 -Checkpoint clean -Scenario <normal|leak> -Out <dir>`(관리자 호스트 PowerShell): `Restore-VMCheckpoint` → `Start-VM` → `Invoke-Command -VMName`이 성공할 때까지 대기 → `Copy-VMFile -FileSource Host`로 설치 파일·`rfspike-snap.exe`·허용 목록·`win-guest-run.ps1`을 `C:\rfspike\`에 복사 → 표준 사용자 `rfspike` 자격으로 `Invoke-Command -VMName ... -FilePath win-guest-run.ps1 -ArgumentList <Scenario>` → 결과 파일을 `Copy-Item -FromSession`으로 `<Out>`에 가져온다. PowerShell Direct가 표준 사용자 자격을 거부하면 오류 메시지를 C7-6 메모에 적고, VM 콘솔에서 `rfspike`로 로그인해 `C:\rfspike\win-guest-run.ps1 <Scenario>`를 직접 실행한 뒤 결과를 가져온다.
`win-guest-run.ps1 <Scenario>`: `rfspike-snap capture --out before.jsonl --reg HKCU --reg HKLM\SOFTWARE --reg HKLM\SYSTEM\CurrentControlSet\Services --dir $env:USERPROFILE --dir $env:ProgramData --dir $env:ProgramFiles --dir ${env:ProgramFiles(x86)} --cmd "cmdkey /list" --cmd "sc.exe query type= service state= all"` → `rfspike-setup.exe /VERYSILENT /CURRENTUSER /TASKS="explorerreplace" /LOG=C:\rfspike\install.log` → `rfspike-ledger simulate-use`(leak이면 `apply all --leak`도) → `"$env:LOCALAPPDATA\Programs\RFSpike\unins000.exe" /VERYSILENT /LOG=C:\rfspike\uninstall.log` → 30초 대기 → 같은 인자로 `after.jsonl` → `rfspike-snap diff --before before.jsonl --after after.jsonl --allow allowlist-windows.txt --id <C7-1|C7-2>`.

- [ ] **Step 8: 허용 목록을 만들고 차이 0을 확인한다(C7-1, C7-5)**

Run: `.\spikes\ledger-vm\scripts\win-vm-pipeline.ps1 -VMName rfspike-win11 -Checkpoint clean -Scenario normal -Out C:\rfspike-out\run1`
첫 실행에서 나온 차이 줄을 보고, OS가 스스로 관리하는 항목만 `allowlist-windows.txt`에 `<kind>:<glob>\t<사유>`로 넣는다(예: `reg:HKLM/SYSTEM/CurrentControlSet/Services/bam/State/UserSettings/**\tOS가 실행 기록을 남기는 BAM`, `file:%USERPROFILE%/NTUSER.DAT*\t레지스트리 하이브 파일`). `RFSpike`, `rfspike`, `.rfspiketest`가 들어간 경로는 절대 허용 목록에 넣지 않는다. 다시 `-Out C:\rfspike-out\run2`로 돌린다.
Expected: `RESULT C7-1 unexpected=0 allowed=<n> verdict=PASS`. C7-5 측정값에 허용 목록 줄 수, 메모에 범주별 개수.

- [ ] **Step 9: 양성 대조를 확인한다(C7-2, RF3)**

Run: `.\spikes\ledger-vm\scripts\win-vm-pipeline.ps1 -VMName rfspike-win11 -Checkpoint clean -Scenario leak -Out C:\rfspike-out\leak`
Expected: 차이 줄 하나 `ADDED reg:HKCU\Software\RFSpikeLeak\Leak`와 `RESULT C7-2 unexpected=1 allowed=<n> verdict=PASS`.

- [ ] **Step 10: 설치 도구 항목을 확인한다(C8)**

- C8-1: run2의 `install.log`에 관리자 승인 요청이 없고 설치 폴더가 `C:\Users\rfspike\AppData\Local\Programs\RFSpike`인지.
- C8-2: run2의 `uninstall.log`에서 `RFSpikeUninstall` 실행 줄이 파일 삭제 줄보다 앞서고, `uninstall-sequence` 출력의 단계 1~4 시각이 순서대로인지, 마지막에 제거 프로그램 등록 키가 없는지(C7-1 결과로 확인).
- C8-3·C8-4: VM 콘솔에서 `rfspike`로 로그인해 대화형으로 `rfspike-setup.exe`를 실행하고 `indexservice`를 체크한다. UAC가 정확히 한 번 뜨고 `sc.exe query rfspike-svc`가 서비스를 보여야 한다. 이어서 "앱 > 설치된 앱"에서 제거할 때 UAC가 한 번 뜨고 `sc.exe query rfspike-svc`가 `1060`(서비스 없음) 오류를 내야 한다.
- C8-5: 같은 대화형 제거에서 체크박스가 기본 해제로 보이는지, 체크하면 문서 폴더에 `RFSpike-backup.zip` 하나만 남는지.
- C8-6: 메모에 Inno Setup 버전과 라이선스 조건(6.5부터 상업 사용자에게 구매를 요청하나 라이선스상 강제는 아님), ARM64는 빌드만 확인했음을 적는다.
- C7-6: 파이프라인 1회 소요 시간, 스냅샷 크기, GitHub 호스티드 러너에 Windows 11 클라이언트 이미지가 없어 셀프 호스티드 Hyper-V 러너가 필요한지 메모에 적는다.

- [ ] **Step 11: 기록하고 커밋한다**

```bash
git add spikes/ledger-vm docs/spikes/m0-report.md
git commit -m "docs(spike): record ledger and Windows VM snapshot pipeline with Inno Setup"
```

---

### Task 20: [macOS 실기 필요] macOS VM 스냅샷 파이프라인(INST-07)

**Files:**
- Modify: `spikes/ledger-vm/src/mac_ops.rs`, `spikes/ledger-vm/src/bin/rfspike-ledger.rs`, `spikes/ledger-vm/Cargo.toml`
- Create: `spikes/ledger-vm/allowlist-macos.txt`, `spikes/ledger-vm/scripts/{mac-bundle.sh, mac-vm-pipeline.sh, mac-guest-run.sh}`
- Modify: `docs/spikes/m0-report.md`(C7-3, C7-4, C7-5, C7-6)

**Interfaces:**
- Consumes: Task 19의 `rfspike_ledger::{snapshot, diff}`와 CLI.
- Produces: `RFSpikeLedger.app`(번들 ID `io.github.rfspike.ledger`), macOS 원장 연산, `RESULT C7-3`, `RESULT C7-4`.

- [ ] **Step 1: macOS 원장 연산을 만든다**

```bash
cargo add --manifest-path spikes/ledger-vm/Cargo.toml objc2 --target 'cfg(target_os = "macos")'
cargo add --manifest-path spikes/ledger-vm/Cargo.toml objc2-foundation --target 'cfg(target_os = "macos")' -F NSString,NSError
cargo add --manifest-path spikes/ledger-vm/Cargo.toml objc2-service-management --target 'cfg(target_os = "macos")' -F SMAppService
```
`mac_ops.rs` 연산(바꾸기 전 값을 원장에 쓰고 적용, 역순 복원):
1. `defaults write -g NSFileViewer -string io.github.rfspike.viewer`(바꾸기 전 값은 `defaults read -g NSFileViewer`, 없으면 없음으로 기록).
2. 키체인: `/usr/bin/security add-generic-password -s rfspike-ledger -a test -w x`(복원은 `delete-generic-password`).
3. LaunchAgent: `SMAppService::agentServiceWithPlistName(ns_string!("io.github.rfspike.ledger.agent.plist"))`의 `registerAndReturnError`(복원 `unregisterAndReturnError`).
4. 로그인 항목: `SMAppService::mainAppService()`의 등록·해제.
`--leak`은 원장 없이 `defaults write io.github.rfspike.leak Leak -int 1`. `simulate-use`는 `~/Library/Application Support/io.github.rfspike.ledger/settings.toml`, `~/Library/Caches/io.github.rfspike.ledger/thumb.bin`, `~/Library/Logs/io.github.rfspike.ledger/app.log`를 만든다. `uninstall-sequence`는 PRD INST-05대로 INST-04 순서의 정리 후, 앱 번들을 `osascript -e 'tell application "Finder" to delete POSIX file "/Applications/RFSpikeLedger.app"'`로 휴지통에 옮긴다.
`mac-bundle.sh`: `RFSpikeLedger.app/Contents/{Info.plist, MacOS/rfspike-ledger, Library/LaunchAgents/io.github.rfspike.ledger.agent.plist}`(agent plist의 `Label` `io.github.rfspike.ledger.agent`, `BundleProgram` `Contents/MacOS/rfspike-ledger`, `ProgramArguments` `["rfspike-ledger","status"]`) → `codesign --force --deep --sign -`.

Run: `cargo build --release --manifest-path spikes/ledger-vm/Cargo.toml && ./spikes/ledger-vm/scripts/mac-bundle.sh`
Expected: `Finished `release` profile [optimized]`와 `spikes/ledger-vm/target/RFSpikeLedger.app`.

- [ ] **Step 2: tart VM을 준비한다**

`brew install cirruslabs/cli/tart && tart clone ghcr.io/cirruslabs/macos-sequoia-base:latest rfspike-mac15-base` (디스크 50GB 이상 여유). 기본 이미지를 한 번 띄워 `ssh-copy-id admin@$(tart ip rfspike-mac15-base)`(암호 `admin`)로 키를 넣고 끈다.
Run: `tart list | grep rfspike-mac15-base`
Expected: `rfspike-mac15-base` 줄.

- [ ] **Step 3: 파이프라인 스크립트를 쓴다**

`mac-vm-pipeline.sh <normal|leak> <out-dir> [--stop-before-uninstall]`(세 번째 인자는 `mac-guest-run.sh`에 그대로 넘긴다): `tart clone rfspike-mac15-base rfspike-run` → `tart run rfspike-run --no-graphics &` → `ssh admin@$(tart ip rfspike-run) true`가 될 때까지 대기 → `scp`로 앱 번들(`ditto`용 zip)·`rfspike-snap`·허용 목록·`mac-guest-run.sh` 복사 → `ssh`로 `mac-guest-run.sh <scenario>` → 결과 `scp` → `tart stop rfspike-run && tart delete rfspike-run`.
`mac-guest-run.sh <scenario>`: `rfspike-snap capture --out before.jsonl --dir ~/Library --dir /Library --dir /Applications --dir ~/.Trash --cmd "defaults export -g -" --cmd "defaults domains" --cmd "security dump-keychain" --cmd "sudo -n sfltool dumpbtm"` → `ditto -x -k RFSpikeLedger.zip /Applications/` → `/Applications/RFSpikeLedger.app/Contents/MacOS/rfspike-ledger apply all`(leak이면 `--leak`) → `simulate-use` → `uninstall-sequence` → 30초 대기 → `after.jsonl` → `rfspike-snap diff ... --allow allowlist-macos.txt --id <C7-3|C7-4>`. 두 번째 인자 `--stop-before-uninstall`이 있으면 `simulate-use` 직후 같은 인자로 `mid.jsonl`을 캡처하고 멈춘다(Step 5용). `defaults domains`의 쉼표 목록은 항목마다 한 줄로 나눠 기록한다.

- [ ] **Step 4: 허용 목록을 만들고 차이 0을 확인한다(C7-3, C7-5)**

Run: `./spikes/ledger-vm/scripts/mac-vm-pipeline.sh normal /tmp/rfspike-out/run1`
Task 19 Step 8과 같은 규칙으로 `allowlist-macos.txt`를 채운다. `~/.Trash/RFSpikeLedger.app*`는 사유 "INST-05: 앱 번들은 휴지통으로 옮기는 것이 명세 동작"으로 넣는다. `io.github.rfspike`가 들어간 그 밖의 경로는 넣지 않는다. 다시 `run2`.
Expected: `RESULT C7-3 unexpected=0 allowed=<n> verdict=PASS`.

- [ ] **Step 5: 개별 연산이 스냅샷에 잡히는지 확인한다**

`./spikes/ledger-vm/scripts/mac-vm-pipeline.sh normal /tmp/rfspike-out/mid --stop-before-uninstall`로 얻은 `before.jsonl`과 `mid.jsonl`을 `rfspike-snap diff --before before.jsonl --after mid.jsonl --allow allowlist-macos.txt --id C7-3`로 비교해, 연산 1~4가 각각 차이 줄로 나타나는지 확인한다(`defaults export`의 `NSFileViewer` 줄, `dump-keychain`의 `rfspike-ledger` 줄, `sfltool dumpbtm`의 `io.github.rfspike.ledger` 줄). 나타나지 않는 연산은 C7-3 메모에 "스냅샷이 감지 못함"으로 적고 판정을 FAIL로 바꾼다.

- [ ] **Step 6: 양성 대조를 확인한다(C7-4, RF3)**

Run: `./spikes/ledger-vm/scripts/mac-vm-pipeline.sh leak /tmp/rfspike-out/leak`
Expected: `ADDED cmd:defaults domains:io.github.rfspike.leak` 한 줄과 `RESULT C7-4 unexpected=1 allowed=<n> verdict=PASS`.

- [ ] **Step 7: 기록하고 커밋한다**

C7-6 메모에 macOS 파이프라인 1회 소요 시간과 tart 라이선스 조건(개인·소규모 조직 무료 범위)을 적는다.

```bash
git add spikes/ledger-vm docs/spikes/m0-report.md
git commit -m "docs(spike): record macOS VM snapshot pipeline"
```

---

### Task 21: 의존성 라이선스 검사와 최소 Rust 버전(8.5) (Linux에서 가능)

**Files:**
- Create: `spikes/deny.toml`
- Modify: `docs/spikes/m0-report.md`(C13)

**Interfaces:**
- Consumes: Task 1~20의 모든 스파이크 `Cargo.toml`과 `Cargo.lock`.
- Produces: C13-1(스파이크별 결과), C13-2(제품 후보 크레이트의 해석된 버전과 rust-version 표). D2가 쓴다.

- [ ] **Step 1: 허용 목록을 쓴다**

`spikes/deny.toml`:

```toml
[graph]
targets = ["x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc", "aarch64-apple-darwin", "x86_64-unknown-linux-gnu"]
all-features = true

[licenses]
confidence-threshold = 0.9
allow = [
  "MIT", "MIT-0", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause",
  "ISC", "Zlib", "Unicode-3.0", "BSL-1.0", "MPL-2.0", "CC0-1.0", "0BSD", "Unlicense",
  "CDLA-Permissive-2.0", "LGPL-2.1-or-later", "LGPL-3.0-or-later", "GPL-3.0-only", "GPL-3.0-or-later",
]
```
`OpenSSL`, `GPL-2.0-only`, `AGPL-3.0`, `SSPL`, 독자 라이선스는 넣지 않는다(PRD 8.5의 GPL-3.0 호환 원칙).

- [ ] **Step 2: 모든 스파이크를 검사한다**

```bash
cargo install --locked cargo-deny
for m in $(git ls-files 'spikes/**/Cargo.toml'); do echo "== $m"; cargo deny --manifest-path "$m" check --config spikes/deny.toml licenses 2>&1 | tail -3; done
```
Expected: 스파이크마다 `licenses ok`. 실패하면 문제 크레이트를 `cargo tree --manifest-path <m> -i <crate>`로 추적해 어떤 의존성이 끌어오는지와 대안 기능 플래그를 C13-1 메모에 적는다. 허용 목록을 넓혀 통과시키지 않는다.

- [ ] **Step 3: 제품 후보 크레이트의 최소 Rust 버전을 적는다**

slint, opendal, zip, sevenz-rust2, windows, objc2, objc2-app-kit, objc2-foundation, russh, russh-sftp, mft, ntfs, fsevent-sys, xxhash-rust 각각에 대해, 그 크레이트를 쓴 스파이크의 `cargo tree --manifest-path <m> -e normal -i <crate> --depth 0`으로 해석된 버전을 얻고 `cargo info <crate>@<버전> | grep -E '^(version|rust-version):'`를 실행한다.
Expected: 크레이트마다 `version:`과 `rust-version:` 줄(없으면 `unknown`). C13-2 측정값에 `crate 버전 → rust-version` 목록과 최댓값을 적는다. 메모에 Slint의 `GPL-3.0-only OR ...` 표기와 프로젝트의 `GPL-3.0-or-later`를 함께 배포하면 배포물은 GPL-3.0 조건을 따른다는 점을 적는다(Q2 참고).

- [ ] **Step 4: 기록하고 커밋한다**

```bash
git add spikes/deny.toml docs/spikes/m0-report.md
git commit -m "docs(spike): record dependency license check and minimum Rust versions"
```

---

### Task 22: 결정 게이트 — 결정 표와 PRD 갱신

**Files:**
- Modify: `docs/spikes/m0-report.md`(D 결정 표, 상태 줄)
- Modify: `docs/PRD.md`(머리말 문서 버전, 5.11 IDX-04, 5.16 WINX-02 표·WINX-07, 5.17 MACX-01, 5.21 I18N-05, 7.5 A11Y-02, 8.1, 8.2 안내 문장(Qt일 때만), 8.4, 8.5, 12, 13, 14)

**Interfaces:**
- Consumes: 보고서 G1~G6, C1~C13 전 행.
- Produces: 채워진 D1~D13, PRD v0.3. M1 계획들이 PRD 8.1·8.4와 D 표를 기준으로 쓴다.

- [ ] **Step 1: 게이트 행에 빈 측정이 없는지 확인한다**

Run: `grep -nE "^\| G[1-5]-[0-9]+ " docs/spikes/m0-report.md | grep -v "(참고)" | grep "| 미측정 |"`
Expected: 출력 없음. 출력이 있으면 그 행의 과제로 돌아가 잰다. 참고 행과 C 표의 `미측정`은 메모에 이유가 있으면 둔다: `grep -nE "\| 미측정 \| \|$" docs/spikes/m0-report.md` → 출력 없음.

- [ ] **Step 2: D 표를 규칙대로 채운다**

| 결정 | 규칙 |
|---|---|
| D1 | G1-1~G1-4, G2-1~G2-6, G3-1~G3-4, G4-1~G4-4, G5-1·G5-2가 모두 PASS면 Slint와 그 렌더러. 하나라도 FAIL이면 Qt 6이고, 근거 열에 FAIL 행 ID를 적는다. G6은 판정에 넣지 않고 보완 계획 유무만 적는다. |
| D2 | C13-2 목록에서 D1·D6·D7·D11·D9·D10이 고른 제품 크레이트들의 rust-version 최댓값. |
| D3 | C2-1~C2-5를 모두 ✔로 만들고 C1-8이 통과한 변형 중 V1 → V2 → V3 순서로 첫 번째. 없으면 가장 많은 행을 덮는 변형과 빠진 행을 적는다. |
| D4 | C3에서 PASS인 방식 중 A → B → C 순서로 첫 번째(PRD "레지스트리 재정의를 우선"). 모두 FAIL이면 "Win+E는 탐색기 유지". |
| D5 | C4-1~C4-5를 모두 PASS로 만든 방식이 있으면 그 방식과 `flash_ms` 중앙값, 일부만이면 "부분 지원(통과 행 나열)", 없으면 "지원 안 함(탐색기가 열림)". |
| D6 | C11-1~C11-3을 모두 ✔로 만든 후보. 둘 이상이면 C11-7·C11-8이 나은 쪽. 없으면 "직접 구현 필요". |
| D7 | C11-4·C11-5를 모두 ✔로 만든 후보. 없으면 "직접 구현 필요". |
| D8 | C8-1~C8-4가 모두 PASS면 Inno Setup. 하나라도 FAIL이면 "재검토"로 적고, 대안(WiX Toolset, NSIS) 스파이크를 별도 계획으로 미룬다. |
| D9 | C9-1과 C9-4가 PASS면 MFT 직접 읽기. 아니면 C9-2·C9-3 중 기준을 만족하는 방식과 PERF-13 미달 사실. |
| D10 | C10-3이 PASS면 `searchfs` 초기 색인 + `getattrlistbulk` 보완, 아니면 `getattrlistbulk`. C10-1이 FAIL이면 그 사실을 함께 적는다. |
| D11 | C6-6~C6-11 중 FAIL인 공급자와 그 대체 라이브러리(SFTP는 C6-12 결과가 PASS면 russh-sftp). |
| D12 | G3 측정값에서 두 OS 각각 통과한 끌어내기·드롭 받기 방식. |
| D13 | C12에서 FAIL인 앱 수가 0이면 "대체 경로는 보조", 1개 이상이면 "서비스 메뉴·URL 스킴을 1차 대안으로 M2에서 구현"(PRD R5 대응). |

보고서 머리의 상태 줄을 `상태: 결정 완료(<날짜>)`로 바꾸고, D 표 아래에 다음 문장을 그대로 넣는다: "M1 UI 계획과 Windows 플랫폼 통합 계획은 이 결정 게이트를 통과한 뒤에만 작성한다. D1이 Qt 6이면 M1 UI 계획보다 먼저 Qt 6 스파이크 계획(게이트 1~6을 같은 기준으로 재측정)을 작성하고 통과시킨다."

- [ ] **Step 3: PRD 8.1을 고친다**

각 줄을 아래 형식으로 바꾼다(`<Dn>`은 D 표의 선택 값).
- `| 언어 | Rust(stable) |` → `| 언어 | Rust(stable, 최소 <D2>) |`
- `| UI | Slint(GPLv3 라이선스로 사용) |` → Slint면 `| UI | Slint(GPLv3 라이선스로 사용, 렌더러 <D1 렌더러>). M0 게이트 통과 |`, Qt면 `| UI | Qt 6. M0 게이트에서 Slint 탈락(<FAIL 행 ID>), Rust 연동 방식은 Qt 스파이크 계획에서 정한다 |`
- `| 원격 저장소 | Apache OpenDAL(FTP, SFTP, WebDAV, Google Drive, OneDrive, Dropbox). SMB는 OS 기본 기능 |` → `| 원격 저장소 | Apache OpenDAL(<D11에 없는 공급자>). 직접 구현: <D11 공급자: 라이브러리>. SMB는 OS 기본 기능 |`
- `| 압축 | libarchive(해제 전반, TAR 계열 압축). ZIP(AES)과 7z 쓰기 라이브러리는 M0에서 비교 후 결정 |` → `| 압축 | libarchive(해제 전반, TAR 계열 압축). ZIP 쓰기는 <D6>, 7z 쓰기는 <D7> |`
- 파일 색인 줄의 `Windows: NTFS MFT·USN 변경 저널(`DeviceIoControl`)`을 `Windows: <D9>와 USN 변경 저널(`DeviceIoControl`)`로, `macOS: `getattrlistbulk` + FSEvents`를 `macOS: <D10> + FSEvents`로.
- `| 설치 | Windows는 Inno Setup 제안([Q7](#13-열린-질문)), macOS는 DMG와 Homebrew cask |` → `| 설치 | Windows는 <D8>(M0 결정), macOS는 DMG와 Homebrew cask |`

D1이 Qt 6이면 8.2의 "M0 게이트에서 Slint가 탈락하면 `rf-ui`만 Qt로 바꾸고 나머지는 그대로 쓴다."를 "M0 게이트에서 Slint가 탈락해 `rf-ui`는 Qt 6로 만들고 나머지는 그대로 쓴다."로 바꾼다.

- [ ] **Step 4: PRD 8.4와 8.5를 고친다**

8.4의 UI 프레임워크 줄 근거 칸 끝에 ` M0 게이트 결과: <G1~G5 요약>(m0-report G1~G6).`를 붙이고(Qt면 선택·근거·대안을 바꾼다), 표 끝에 줄을 더한다: 폴더 열기 등록(D3), Win+E(D4), `/select`(D5), ZIP·7z 쓰기(D6·D7), Windows 설치 도구(D8), 초기 색인(D9·D10), 원격 공급자 직접 구현(D11), OS 드래그 앤 드롭 계층(D12). 각 줄은 `| 결정 | 선택 | 근거(보고서 행 ID) | 검토한 대안 |`.
8.5 표에 D6·D7·D11에서 고른 라이브러리 줄을 `| <이름> | <라이선스> | 예 | 내장 |`으로 더하고, 표 아래 문장 "M0에서 cargo-deny로 전체 의존성 트리의 라이선스를 검사하고, 이후 CI에서 계속 검사한다."를 "M0에서 cargo-deny로 전체 의존성 트리의 라이선스를 검사했고(m0-report C13), 이후 CI에서 계속 검사한다."로 바꾼다.

- [ ] **Step 5: PRD 12와 13을 고친다**

12장: 아래 줄의 대응 칸 끝에 ` M0 결과: <한 문장>(m0-report <행 ID>).`를 붙이고 가능성 칸을 규칙대로 바꾼다.
- R1: D1이 Slint면 가능성 `낮음`, Qt면 `높음(M0에서 확인)`.
- R4: D4가 방식 A면 그대로, B·C면 `중간`, "탐색기 유지"면 `높음(M0에서 확인)`.
- R5: C12의 FAIL 앱 수가 0이면 `낮음`, 1~2개면 `중간`, 3개 이상이면 `높음`.
- R9: 대응 칸의 "M0에서 검증하고, 부족한 공급자는 그 공급자만 직접 구현한다."를 "M0 결과: <D11>만 직접 구현한다(m0-report C6)."로 바꾸고, D11이 비어 있지 않으면 가능성 `높음(M0에서 확인)`.
- R16: 대응 칸의 "`searchfs` 활용은 M0에서 검토한다."를 "M0 결과: <D10>(m0-report C10-3)."로 바꾸고, C10-1이 PASS면 가능성 `낮음`.
13장: Q7 줄을 `| Q7 | Windows 설치 도구 최종 선택 | 결정: <D8>(m0-report C8). 원장 복원은 앱이 담당 | M0(완료) |`로 바꾼다. M0에서 새로 생긴 질문이 있으면 Q10부터 같은 형식(제안, 결정 시점)으로 더한다.

- [ ] **Step 6: 결정을 미뤄 둔 요구사항 문장과 머리말·결정 기록을 고친다**

- 머리말 `| 문서 버전 | 0.2 (초안, 빠른 파일 검색 추가) |` → `| 문서 버전 | 0.3 (M0 기술 검증 결과 반영) |`
- IDX-04의 "더 빠른 초기 색인 방법(카탈로그 검색 `searchfs`)이 APFS에서 쓸 만한지는 M0에서 확인한다." → "초기 색인은 <D10>을 쓴다(M0 결과, m0-report C10)."
- WINX-07의 "어느 방식이 Windows 11에서 안정적으로 동작하는지는 M0에서 검증한다." → "M0 결과 <D4>를 쓴다(m0-report C3)."
- WINX-02 표의 `/select` 행 처리 칸 "최선 노력. 방식은 M0에서 검증하고, 실패하면 탐색기가 열린다." → "<D5>(M0 결정, m0-report C4). 실패하면 탐색기가 열린다."
- MACX-01 끝에 "M0 호환성 표: m0-report C12." 를 붙인다.
- I18N-05의 "M0 검증 게이트 항목이다." → "M0 검증 게이트 항목이다(결과: m0-report G2)."
- A11Y-02의 "Slint의 지원 수준은 M0에서 확인한다." → "M0 결과: <G6 요약>(m0-report G6)."
- 14장 표 끝에 D1~D13마다 `| <결정 날짜> | <결정 이름> | <선택>(m0-report D<n>) |` 줄을 더한다.

Run: `grep -n "M0에서" docs/PRD.md`
Expected: 8.5의 cargo-deny 문장 한 줄만 나온다.

- [ ] **Step 7: 커밋한다**

```bash
git add docs/spikes/m0-report.md docs/PRD.md
git commit -m "docs(prd): apply M0 spike decisions"
```
