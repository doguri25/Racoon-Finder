# Windows installer

`racoon-finder.iss` is an [Inno Setup 6](https://jrsoftware.org/isinfo.php) script. CI builds it
twice per run (job `installer` in `.github/workflows/demo.yml`), once per CPU architecture, from the
`racoon-finder.exe` of the portable zip. The installer is **per user** (no administrator prompt) and
installs to `%LOCALAPPDATA%\Programs\RacoonFinder`.

## Build locally

```bat
ISCC.exe /DAppVersion=0.5.0 /DArch=x64 /DSource=C:\path\to\folder installer\windows\racoon-finder.iss
```

| Define | Meaning |
| --- | --- |
| `AppVersion` | plain `x.y.z`, the workspace version (CI reads it from the root `Cargo.toml`) |
| `Arch` | `x64` (accepts x64 and Windows on ARM through emulation) or `arm64` (ARM only) |
| `Source` | folder that holds `racoon-finder.exe` (no `portable` file is installed even if one is there) |

The output is `RacoonFinder-setup-<AppVersion>-windows-<Arch>.exe` in `installer\windows\Output`
(`/O<dir>` changes it). Inno Setup **6.3 or newer** is needed: 6.3 added the `x64compatible`
identifier and the official Korean translation, which ships as `Languages\Korean.isl` and is used
as it is. The script stops with an error on an older compiler or when that file is missing. CI uses
the Inno Setup on the runner image if it is 6.3 or newer, else installs a pinned 6.x with Chocolatey
(never the latest: a 7.x installs to a different folder).

## Languages

The installer and the uninstaller speak English (`compiler:Default.isl`) and Korean
(`compiler:Languages\Korean.isl`). Which one is Windows' user interface language
(`LanguageDetectionMethod=uilanguage`): Korean on a Korean Windows, English on an English one,
without asking; on any other Windows the list of languages shows with English chosen
(`ShowLanguageDialog=auto`, English is the first entry), as the app itself speaks English on any
system that is not Korean. An upgrade keeps the language of the install before it
(`UsePreviousLanguage`, on by default: an upgrade of 0.4.0, which spoke only Korean, stays Korean),
and the uninstaller speaks the language the install chose. The script's own
words (the desktop shortcut task, the launch box, the uninstaller's question and messages) are in
`[CustomMessages]`, one line per language (`english.RfBackupAsk=…`, `korean.RfBackupAsk=…`): a new
language needs its `[Languages]` entry and a line of each. The app's own language is a setting of the
app (View ▸ Theme, `docs/i18n.md`), not the installer's.

## The AppId rule

`AppId` in the script is a GUID generated once. **It must never change**: Windows and later
installers use it to find the installed copy, so a changed id installs a second copy next to the old
one instead of upgrading, and leaves two entries in the Apps list.

## What the uninstaller does

1. Asks "삭제하기 전에 설정을 백업할까요?" ("Back up the settings before uninstalling?", default: No). Yes copies `settings.toml` to
   `Documents\RacoonFinder-settings-backup.toml`. A silent uninstall does not ask: no backup.
2. Runs `racoon-finder.exe --cleanup [--backup-settings <file>]` (from `[Code]`, before the files are
   deleted): it asks a running app to quit (waits up to 30 s), puts back what the app changed, and
   deletes the settings, cache (thumbnails too), logs and temporary files. Exit codes: 0 done,
   3 app still running, 4 backup failed (nothing deleted), 5 partly done. A nonzero code does not stop
   the uninstall; a message names the code (none in a silent uninstall; the code goes to the log).
3. Removes what Inno installed: the exe, the Start menu and desktop shortcuts, the install folder,
   the Apps list entry.

It touches nothing else. A portable install (a `portable` file next to the exe) is not affected, and
`--cleanup` run from a portable folder does nothing.

## Test

```bat
RacoonFinder-setup-0.5.0-windows-x64.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER /DIR=C:\Temp\rfinst /LOG=install.log
C:\Temp\rfinst\unins000.exe /VERYSILENT /SUPPRESSMSGBOXES /LOG=uninstall.log
```

The silent uninstall runs `--cleanup` without the backup question. The uninstaller returns before it
has finished (it re-launches itself from a temp folder), so wait until the folder is gone. CI does this
for the x64 installer. The manual list is in the root `README.md`, "실제 PC에서 손으로 확인할 것".

## Known limits

- Not code-signed: SmartScreen warns ("Windows의 PC 보호").
- No option yet to replace Explorer or to start resident at sign-in.
- The script could not be compiled by the author's tooling; CI is its first compile. That holds for
  the languages too: the English pages and messages have not been seen on a real Windows yet.
