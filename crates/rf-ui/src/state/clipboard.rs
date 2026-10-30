//! The system clipboard (DND-10, DND-11): copying and cutting the items of a pane, pasting
//! files into a folder, and copying the items' paths as text (경로 복사, OPS-07). A child of
//! [`state`](super), so these methods see the fields of [`AppState`].
//!
//! The clipboard itself is the system's, which only the clipboard's worker touches
//! ([`ClipboardWork`], carried out one at a time and in order by the controller's worker, so that
//! a paste right after a copy reads that copy). The window keeps what it put there last
//! ([`Own`]): which items, whether they were cut, and the clipboard's change count after the
//! put. While that count is the clipboard's, the cut items show dimmed in the panes; once
//! another app (or this one) has put something else there, the clipboard's content is the
//! truth and the dimming goes, as it does on a new copy or cut and after the paste that moved
//! them.
//!
//! Esc with nothing selected cancels a cut ([`AppState::escape`]), and then nothing may move
//! the items any more: the clipboard is emptied if it still holds them, a paste of what has
//! that change count pastes nothing (another emptying goes with it), and an emptying that fails
//! (another app holds the clipboard) is tried again at the next looks at the change count. When
//! it fails every time, a notice says that a paste in another app could still move them.
//!
//! A paste reads the clipboard afresh ([`ClipboardWork::ReadFiles`]) and submits a transfer of
//! what it holds into the folder: a move when the clipboard says so (File Explorer's cut, this
//! window's on Windows), when this window's own cut is still what the clipboard holds (macOS,
//! whose pasteboard does not say), or with Finder's ⌥⌘V; a copy otherwise. Names that are
//! taken get a number (`파일 (2).txt`, [`ConflictPolicy::AutoRename`]), and so does a copy into
//! the folder the items are in. A cut never deletes anything itself: only the move job does,
//! item by item, and once it has moved everything the clipboard is emptied (as File Explorer
//! does), if it still holds what the paste read. What a paste made is selected in the pane it
//! was asked in, as it shows there.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use rf_core::{EntryKind, Os, VPath};
use rf_jobs::{
    ConflictPolicy, JobReport, JobSpec, Outcome, TransferMode, TransferOptions, TransferSpec,
};
use rf_platform::clipboard::{ClipboardFiles, DropEffect, is_device_path};

use super::jobs::submit;
use super::{AppState, Effect, Paging};
use crate::keys::{Command, Mods};
use crate::words;

/// How the paths of items go to the clipboard as text (경로 복사, OPS-07).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PathForm {
    /// The path as the system writes it: `C:\문서\a.txt`, `/Users/me/a.txt`.
    Full,
    /// The item's name alone: `a.txt`.
    Name,
    /// The path in double quotes, as a shell reads it back whole: `"C:\문서\a b.txt"`.
    Quoted,
    /// On Windows, the path through the network share of a mapped drive
    /// (`\\server\share\a.txt` for `Z:\a.txt`); the full path for any other.
    Unc,
    /// The path with `/` between its names: `C:/문서/a.txt`.
    Slashes,
    /// A `file:` URL: `file:///C:/%EB%AC%B8%EC%84%9C/a.txt`.
    FileUrl,
}

impl PathForm {
    /// Every form, in the order of the menus; a form's place is its number there.
    pub const ALL: [PathForm; 6] = [
        PathForm::Full,
        PathForm::Name,
        PathForm::Quoted,
        PathForm::Unc,
        PathForm::Slashes,
        PathForm::FileUrl,
    ];

    /// The number of the form in the menus (`copy-path-<number>`).
    pub fn number(self) -> i32 {
        match self {
            PathForm::Full => 0,
            PathForm::Name => 1,
            PathForm::Quoted => 2,
            PathForm::Unc => 3,
            PathForm::Slashes => 4,
            PathForm::FileUrl => 5,
        }
    }

    /// The form numbered `number`; `None` for any other number.
    pub fn from_number(number: i32) -> Option<PathForm> {
        PathForm::ALL
            .into_iter()
            .find(|form| form.number() == number)
    }

    /// The form the system's key for copying paths gives (Ctrl+Shift+C, ⌥⌘C): File Explorer's
    /// 경로로 복사 puts the paths in quotes, Finder's 경로 이름 복사 does not.
    pub fn of_key(os: Os) -> PathForm {
        if os == Os::MacOs {
            PathForm::Full
        } else {
            PathForm::Quoted
        }
    }
}

/// The text 경로 복사 puts on the clipboard for `paths` (each as the system `os` writes it) in
/// `form`: one a line, with `\r\n` between them on Windows and `\n` elsewhere. For
/// [`PathForm::Unc`] the caller has put each mapped drive's share in its place (the worker asks
/// the system), so the paths are taken as they are.
///
/// A name with a line break in it (Linux and macOS allow one) spans two lines in the forms that
/// take the path as it is (full, name, UNC, slashes): such text is no list of one path a line.
/// The quoted form keeps it inside its quotes, where a shell still reads one word, and the file
/// URL encodes it (`%0A`).
pub fn path_text(paths: &[String], form: PathForm, os: Os) -> String {
    let newline = if os == Os::Windows { "\r\n" } else { "\n" };
    paths
        .iter()
        .map(|path| path_line(path, form, os))
        .collect::<Vec<_>>()
        .join(newline)
}

/// One path of [`path_text`].
fn path_line(path: &str, form: PathForm, os: Os) -> String {
    let windows = os == Os::Windows;
    match form {
        PathForm::Slashes if windows => path.replace('\\', "/"),
        PathForm::Full | PathForm::Unc | PathForm::Slashes => path.to_owned(),
        PathForm::Name => name_of(path, windows).to_owned(),
        // Windows names hold no `"`.
        PathForm::Quoted if windows => format!("\"{path}\""),
        // A POSIX shell reads everything between single quotes as it is; a quote of the name
        // closes them, goes escaped, and opens them again.
        PathForm::Quoted => format!("'{}'", path.replace('\'', "'\\''")),
        PathForm::FileUrl => file_url(path, os),
    }
}

/// The last name of `path`; the whole path for a root (`C:\`, `/`, `\\server\share\`).
fn name_of(path: &str, windows: bool) -> &str {
    let separators: &[char] = if windows { &['\\', '/'] } else { &['/'] };
    match path.rfind(separators) {
        Some(at) if at + 1 < path.len() => &path[at + 1..],
        _ => path,
    }
}

/// The `file:` URL of `path`, as the system `os` writes it: `C:\a b\c.txt` is
/// `file:///C:/a%20b/c.txt`, `\\server\share\x` is `file://server/share/x`, and `/a b` is
/// `file:///a%20b`. Every byte of a name that a URL's path does not take as it is (spaces,
/// `%`, `#`, `?`, letters outside ASCII, …) is percent-encoded, so that other apps read it and
/// [`read_file_url`](super::read_file_url) reads back the path. A backslash in a name, which
/// only Linux and macOS allow, is `%5C`.
pub fn file_url(path: &str, os: Os) -> String {
    let mut url = String::from("file://");
    if os != Os::Windows {
        encode_path(&mut url, path.split('/'));
        return url;
    }
    if let Some(share) = path.strip_prefix("\\\\") {
        let (host, rest) = share.split_once('\\').unwrap_or((share, ""));
        encode_name(&mut url, host);
        url.push('/');
        encode_path(&mut url, rest.split('\\'));
        return url;
    }
    url.push('/');
    encode_path(&mut url, path.split('\\'));
    url
}

/// Writes `names` to `url` with `/` between them, each percent-encoded.
fn encode_path<'a>(url: &mut String, names: impl Iterator<Item = &'a str>) {
    for (i, name) in names.enumerate() {
        if i > 0 {
            url.push('/');
        }
        encode_name(url, name);
    }
}

/// Writes `name` to `url`: the characters a URL's path takes as they are (RFC 3986 `pchar`:
/// letters and digits of ASCII, `-._~`, `!$&'()*+,;=`, `:` and `@`), and `%XX` for each byte
/// of the UTF-8 of any other.
fn encode_name(url: &mut String, name: &str) {
    for byte in name.bytes() {
        let plain = byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@".contains(&byte);
        if plain {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
}

/// What the clipboard's worker is asked to do, one request at a time, in the order asked
/// ([`Effect::Clipboard`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardWork {
    /// Put the items `paths` on the clipboard, to be copied, or moved when `cut`; then hand the
    /// clipboard's change count (or why not) to [`AppState::clipboard_put`] with `token`.
    PutFiles {
        token: u64,
        paths: Vec<VPath>,
        cut: bool,
    },
    /// Read the files on the clipboard for a paste, then hand them to
    /// [`AppState::clipboard_read`] with `token`.
    ReadFiles { token: u64 },
    /// Put the paths of `paths` on the clipboard as text in `form` ([`path_text`]); tell
    /// [`AppState::paths_copied`] how it went.
    PutPaths { paths: Vec<VPath>, form: PathForm },
    /// Put `text` on the clipboard (a checksum of the properties window); tell
    /// [`AppState::checksum_copied`] with `token` how it went.
    PutText { token: u64, text: String },
    /// Empty the clipboard if its change count is still this one (nothing came since); tell
    /// [`AppState::clipboard_cleared`] how it went.
    ClearIf(u64),
}

/// Why the clipboard did not do what it was asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardFailure {
    /// The system has no clipboard for it (text on Linux).
    Unsupported,
    /// Another app held it, or the system refused.
    Unavailable,
}

impl From<&std::io::Error> for ClipboardFailure {
    fn from(error: &std::io::Error) -> ClipboardFailure {
        if error.kind() == std::io::ErrorKind::Unsupported {
            ClipboardFailure::Unsupported
        } else {
            ClipboardFailure::Unavailable
        }
    }
}

/// What the window put on the clipboard last, as long as the clipboard may still hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Own {
    /// The request that put it.
    token: u64,
    paths: Vec<VPath>,
    /// They were cut, to be moved: they show dimmed.
    cut: bool,
    /// The clipboard's change count after the put; `None` while the put is on its way.
    change: Option<u64>,
}

/// Where a paste whose read is on its way goes.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PasteTo {
    /// The pane it was asked in.
    side: usize,
    folder: VPath,
    /// ⌥⌘V: the items are moved whatever the clipboard says.
    force_move: bool,
    /// The tab of that pane and its selection epoch when the paste was asked: what the paste
    /// makes is selected only while both are still so ([`AppState::pasted_in`]).
    mark: (u64, u64),
}

/// How many times the clipboard is asked to drop a cut cancelled with Esc before a notice says
/// that it still holds the cut.
pub const CLEAR_TRIES: u32 = 4;

/// The emptying of the clipboard after a cut was cancelled, until it is seen to work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Clearing {
    /// The change count of the cut.
    change: u64,
    /// The tries that failed so far.
    failed: u32,
}

/// What the window keeps of the clipboard.
#[derive(Debug, Default)]
pub(super) struct ClipWork {
    own: Option<Own>,
    /// The token of the last request.
    token: u64,
    /// A cut cancelled (Esc) while its put was on its way: the clipboard is emptied once the
    /// put has told its count, if nothing came since. A later put does not reset it: that put
    /// may fail and leave the cut on the clipboard, and the token matches its own put alone.
    cancelled: Option<u64>,
    /// The change count of the last cut cancelled with Esc: what the clipboard holds with it is
    /// never pasted (unless ⌥⌘V asks for a move). It is never reset: it matches that cut's
    /// count alone, which nothing else on the clipboard has.
    cancelled_change: Option<u64>,
    /// The emptying of that cut, until it worked, or until a later put of the window worked and
    /// so replaced the cut on the clipboard. A put that fails leaves it, and its warning, alive.
    clearing: Option<Clearing>,
    /// The pastes whose read is on its way, by token.
    pastes: HashMap<u64, PasteTo>,
    /// The moves pastes submitted that have not ended, with the change count of what the
    /// clipboard held when they read it.
    moves: Vec<(JobSpec, u64)>,
    /// The pastes submitted that have not ended, with the pane each was asked in and that
    /// pane's tab and selection epoch then: what one makes is selected there
    /// ([`AppState::pasted_in`]).
    pasted: Vec<(JobSpec, usize, (u64, u64))>,
}

impl ClipWork {
    fn next_token(&mut self) -> u64 {
        self.token += 1;
        self.token
    }
}

impl AppState {
    /// The names of the items in `folder` that were cut and that the clipboard may still hold:
    /// they show dimmed (DND-10). Empty when nothing is cut.
    pub fn cut_names(&self, folder: &VPath) -> HashSet<OsString> {
        let Some(own) = self.clipboard.own.as_ref().filter(|own| own.cut) else {
            return HashSet::new();
        };
        own.paths
            .iter()
            .filter(|path| path.parent().as_ref() == Some(folder))
            .filter_map(|path| path.file_name().map(std::ffi::OsStr::to_os_string))
            .collect()
    }

    /// Whether the window waits to see the clipboard change: items it cut are on it. The
    /// controller then asks the clipboard's change count now and then
    /// ([`AppState::clipboard_polled`]).
    /// It also waits while a cancelled cut is still to be emptied: each look tries again.
    pub fn clipboard_watched(&self) -> bool {
        self.clipboard.clearing.is_some()
            || self
                .clipboard
                .own
                .as_ref()
                .is_some_and(|own| own.cut && own.change.is_some())
    }

    /// Ctrl+C, Ctrl+X (and their menu items): the items of pane `side` (the selected rows, or
    /// the row the user put the cursor on) go on the clipboard, to be copied or, when `cut`,
    /// moved by a paste. Nothing for a pane without such an item. They replace any items cut
    /// before, which no longer show dimmed.
    pub(super) fn put_on_clipboard(&mut self, side: usize, cut: bool) -> Vec<Effect> {
        let paths: Vec<VPath> = self
            .targets(side)
            .into_iter()
            .filter(|path| path.as_local().is_some())
            .collect();
        if paths.is_empty() {
            return Vec::new();
        }
        let token = self.clipboard.next_token();
        // A cut cancelled before is still guarded: this put may fail and leave it there.
        self.clipboard.own = Some(Own {
            token,
            paths: paths.clone(),
            cut,
            change: None,
        });
        vec![Effect::Clipboard(ClipboardWork::PutFiles {
            token,
            paths,
            cut,
        })]
    }

    /// The worker has put the items of request `token` on the clipboard (`Ok`, with the
    /// clipboard's change count after it), or could not: the items no longer show dimmed, and a
    /// notice says why. Nothing for a request that a later one has replaced.
    pub fn clipboard_put(
        &mut self,
        token: u64,
        result: Result<u64, ClipboardFailure>,
    ) -> Vec<Effect> {
        if self.clipboard.cancelled == Some(token) {
            self.clipboard.cancelled = None;
            // Esc came while it was on its way: the cut goes from the clipboard too.
            return match result {
                Ok(change) => self.drop_cancelled(change),
                Err(_) => Vec::new(),
            };
        }
        let Some(own) = self.clipboard.own.as_mut().filter(|own| own.token == token) else {
            return Vec::new();
        };
        match result {
            Ok(change) => {
                own.change = Some(change);
                // The put replaced a cancelled cut the clipboard still held: nothing is left
                // to empty.
                self.clipboard.clearing = None;
                Vec::new()
            }
            Err(failure) => {
                self.clipboard.own = None;
                vec![Effect::Notice(words::clipboard_failed(failure).to_owned())]
            }
        }
    }

    /// The clipboard's change count is now `change`: when it is no longer the count of what
    /// the window put there, another app has put something else, and the items cut no longer
    /// show dimmed. A cancelled cut the clipboard still holds is asked to go again, and one it
    /// no longer holds needs nothing more. `None` when nothing changed; else what to carry out,
    /// and the window shows the change. A count the system does not tell changes nothing.
    pub fn clipboard_polled(&mut self, change: Option<u64>) -> Option<Vec<Effect>> {
        let now = change?;
        let mut changed = false;
        if self
            .clipboard
            .own
            .as_ref()
            .is_some_and(|own| own.change.is_some_and(|own| own != now))
        {
            changed = self.clipboard.own.take().is_some_and(|own| own.cut);
        }
        let mut effects = Vec::new();
        match self.clipboard.clearing {
            Some(clearing) if clearing.change == now => {
                effects.push(Effect::Clipboard(ClipboardWork::ClearIf(now)));
            }
            Some(_) => self.clipboard.clearing = None,
            None => {}
        }
        (changed || !effects.is_empty()).then_some(effects)
    }

    /// The change count of a cut cancelled with Esc that the clipboard may still hold as the
    /// app ends: its emptying is on its way, or failed fewer than [`CLEAR_TRIES`] times (after
    /// that, a notice has told the user). The app's end tries once more to empty it, for a
    /// moment ([`crate::controller::CLEAR_AT_END`]), so that no paste in another app moves the
    /// items after all. `None` when nothing waits to be emptied.
    pub fn cut_left_on_clipboard(&self) -> Option<u64> {
        self.clipboard.clearing.map(|clearing| clearing.change)
    }

    /// The worker tried to empty the clipboard if its change count was still `change`:
    /// `Ok(true)` it did, `Ok(false)` something else is there now, or it could not. For a cut
    /// cancelled with Esc, a failure is tried again at the next look at the change count
    /// ([`AppState::clipboard_polled`]), [`CLEAR_TRIES`] times in all; then a notice says the
    /// clipboard still holds the cut. Any other emptying (after a move) needs no answer.
    pub fn clipboard_cleared(
        &mut self,
        change: u64,
        result: Result<bool, ClipboardFailure>,
    ) -> Vec<Effect> {
        let Some(clearing) = self
            .clipboard
            .clearing
            .as_mut()
            .filter(|clearing| clearing.change == change)
        else {
            return Vec::new();
        };
        if result.is_ok() {
            self.clipboard.clearing = None;
            return Vec::new();
        }
        clearing.failed += 1;
        if clearing.failed < CLEAR_TRIES {
            return Vec::new();
        }
        self.clipboard.clearing = None;
        vec![Effect::Notice(words::clipboard_not_emptied().to_owned())]
    }

    /// Esc in the panes (the key alone: `keys::cancels_cut`). With items selected it clears the
    /// selection; with nothing selected, while items the window cut are on the clipboard, it
    /// cancels the cut as File Explorer's Esc does: they no longer show dimmed, and the
    /// clipboard is emptied if it still holds them, so that no paste moves them (see the
    /// module). The other ways to clear the selection (the 선택 menu, ⌥⌘A) cancel no cut.
    /// `None` as [`AppState::key`] gives it.
    pub fn escape(&mut self, paging: Paging, now: Instant) -> Option<Vec<Effect>> {
        let cancels =
            self.modal.is_none() && !self.quitting && self.tab(self.active).selection().is_empty();
        let mut effects = if cancels {
            self.cancel_cut().unwrap_or_default()
        } else {
            Vec::new()
        };
        effects.extend(self.key(Command::ClearSelection, Mods::NONE, paging, now)?);
        Some(effects)
    }

    /// The cut the window put on the clipboard is cancelled; `None` when nothing is cut.
    fn cancel_cut(&mut self) -> Option<Vec<Effect>> {
        let own = self.clipboard.own.take_if(|own| own.cut)?;
        if let Some(change) = own.change {
            return Some(self.drop_cancelled(change));
        }
        // The put tells its count later: the clipboard is emptied then.
        self.clipboard.cancelled = Some(own.token);
        Some(Vec::new())
    }

    /// The cancelled cut with the change count `change` goes from the clipboard, and nothing
    /// pastes it any more.
    fn drop_cancelled(&mut self, change: u64) -> Vec<Effect> {
        self.clipboard.cancelled_change = Some(change);
        if self
            .clipboard
            .clearing
            .is_none_or(|clearing| clearing.change != change)
        {
            self.clipboard.clearing = Some(Clearing { change, failed: 0 });
        }
        vec![Effect::Clipboard(ClipboardWork::ClearIf(change))]
    }

    /// Ctrl+V (⌘V; ⌥⌘V with `force_move`): pastes what the clipboard holds into the folder of
    /// pane `side`, once the worker has read it.
    pub(super) fn paste(&mut self, side: usize, force_move: bool) -> Vec<Effect> {
        let folder = self.tab(side).location().clone();
        self.read_for_paste(side, folder, force_move)
    }

    /// 붙여넣기 (or 여기로 이동, `force_move`) in the menu of a row of the active pane: into the
    /// folder of that row when it is a folder, as File Explorer pastes into a folder
    /// right-clicked, and else into the pane's folder. The row is the one the menu was opened
    /// on (the cursor the user put there): a cursor the list placed is not, and the paste
    /// goes into the pane's folder then.
    pub fn paste_from_menu(&mut self, force_move: bool) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        let side = self.active;
        let tab = self.tab(side);
        let row_folder = tab
            .selection()
            .user_cursor()
            .and_then(|row| tab.visible().get(row))
            .filter(|entry| entry.kind == EntryKind::Dir)
            .and_then(|entry| tab.location().join(&entry.name).ok());
        let folder = row_folder.unwrap_or_else(|| tab.location().clone());
        self.read_for_paste(side, folder, force_move)
    }

    /// Asks the worker for the clipboard's files, for a paste into `folder` asked in pane
    /// `side`.
    fn read_for_paste(&mut self, side: usize, folder: VPath, force_move: bool) -> Vec<Effect> {
        if folder.as_local().is_none() {
            return Vec::new();
        }
        let token = self.clipboard.next_token();
        let tab = self.tab(side);
        let mark = (tab.id(), tab.selection_epoch());
        self.clipboard.pastes.insert(
            token,
            PasteTo {
                side,
                folder,
                force_move,
                mark,
            },
        );
        vec![Effect::Clipboard(ClipboardWork::ReadFiles { token })]
    }

    /// What the worker read from the clipboard for the paste `token`: its files are copied or
    /// moved into the paste's folder (see the module), or a notice says why nothing is. Items
    /// the window cut before, which the clipboard holds no longer, stop showing dimmed.
    pub fn clipboard_read(
        &mut self,
        token: u64,
        result: Result<Option<ClipboardFiles>, ClipboardFailure>,
    ) -> Vec<Effect> {
        let Some(to) = self.clipboard.pastes.remove(&token) else {
            return Vec::new();
        };
        if self.quitting {
            return Vec::new();
        }
        let files = match result {
            Ok(Some(files)) => files,
            Ok(None) => return vec![Effect::Notice(words::nothing_to_paste().to_owned())],
            Err(failure) => {
                return vec![Effect::Notice(words::clipboard_failed(failure).to_owned())];
            }
        };
        // A cut cancelled with Esc that the clipboard still holds (its emptying is on its way,
        // or failed) pastes nothing; Finder's ⌥⌘V, a move asked for by name, still moves.
        if self.clipboard.cancelled_change == Some(files.change) && !to.force_move {
            let mut effects = vec![Effect::Notice(words::nothing_to_paste().to_owned())];
            effects.extend(self.drop_cancelled(files.change));
            return effects;
        }
        // What the window put is the truth only while the clipboard holds it; a put still on
        // its way tells its count later.
        let ours = self
            .clipboard
            .own
            .as_ref()
            .and_then(|own| own.change)
            .map(|change| change == files.change);
        if ours == Some(false) {
            self.clipboard.own = None;
        }
        let our_cut = ours == Some(true) && self.clipboard.own.as_ref().is_some_and(|own| own.cut);
        let moves = to.force_move
            || match files.effect {
                Some(effect) => effect == DropEffect::Move,
                None => our_cut,
            };
        // What is no local path, and what the system's reading left out already (macOS: a
        // file URL of another computer).
        let (sources, left_out) = local_paths(&files.paths);
        let left_out = left_out + files.skipped;
        if sources.is_empty() {
            return vec![Effect::Notice(words::nothing_to_paste().to_owned())];
        }
        let mode = if moves {
            TransferMode::Move
        } else {
            TransferMode::Copy
        };
        let mut effects = self.paste_spec(&to, sources, mode, files.change);
        // The others go; the user is told that some did not.
        let submitted = effects
            .iter()
            .any(|effect| matches!(effect, Effect::Job(_)));
        if left_out > 0 && submitted {
            effects.push(Effect::Notice(words::paste_left_out(left_out)));
        }
        effects
    }

    /// The paste of `sources` into `to`, to `mode` them, of the clipboard content with the
    /// change count `change`: checked, then submitted.
    fn paste_spec(
        &mut self,
        to: &PasteTo,
        mut sources: Vec<VPath>,
        mode: TransferMode,
        change: u64,
    ) -> Vec<Effect> {
        // Never a folder into itself or a folder inside it; the job would refuse that item, and
        // the paste is refused whole, as one that cannot be what was meant.
        if sources.iter().any(|source| to.folder.starts_with(source)) {
            return vec![Effect::Notice(words::paste_into_itself().to_owned())];
        }
        if mode == TransferMode::Move {
            // A move to where an item is already does nothing (DND-09).
            sources.retain(|source| source.parent().as_ref() != Some(&to.folder));
            if sources.is_empty() {
                return vec![Effect::Notice(words::paste_already_here().to_owned())];
            }
            // The same cut pasted twice before the first move ended would move nothing the
            // second time but fail for every item.
            if self
                .clipboard
                .moves
                .iter()
                .any(|(_, moving)| *moving == change)
            {
                return vec![Effect::Notice(words::paste_moving().to_owned())];
            }
        }
        let options = TransferOptions {
            conflict: ConflictPolicy::AutoRename,
            ..TransferOptions::default()
        };
        match TransferSpec::into_dir(mode, &sources, &to.folder, options) {
            Ok(spec) => {
                let spec = JobSpec::Transfer(spec);
                if mode == TransferMode::Move {
                    self.clipboard.moves.push((spec.clone(), change));
                }
                self.clipboard.pasted.push((spec.clone(), to.side, to.mark));
                vec![submit(spec, Some(to.side))]
            }
            Err(error) => vec![Effect::Notice(words::cannot_transfer(error.kind()))],
        }
    }

    /// A job ended with `report`: when it is a move a paste submitted and it moved every item
    /// (none failed, and it did not copy instead), the clipboard is emptied if it still holds
    /// what the paste read, and the items the window cut no longer show dimmed (they have
    /// moved). Any other end leaves the clipboard as it is.
    pub(super) fn paste_ended(&mut self, report: &JobReport) -> Vec<Effect> {
        let Some(at) = self
            .clipboard
            .moves
            .iter()
            .position(|(spec, _)| *spec == report.spec)
        else {
            return Vec::new();
        };
        let (_, change) = self.clipboard.moves.remove(at);
        let moved_all = report.outcome == Outcome::Completed
            && report.failures.is_empty()
            && !report.copied_instead_of_moved;
        if !moved_all {
            return Vec::new();
        }
        if self
            .clipboard
            .own
            .as_ref()
            .is_some_and(|own| own.change == Some(change))
        {
            self.clipboard.own = None;
        }
        vec![Effect::Clipboard(ClipboardWork::ClearIf(change))]
    }

    /// The pane a paste that ended with `report` was asked in, where what it made is selected
    /// (as File Explorer selects what it pastes), with the tab and the selection epoch it had
    /// when the paste was asked: a selection the user changed since, even while the clipboard
    /// was read, stays. `None` for a job that is no paste. Each paste is told of once.
    pub(super) fn pasted_in(&mut self, report: &JobReport) -> Option<(usize, (u64, u64))> {
        let at = self
            .clipboard
            .pasted
            .iter()
            .position(|(spec, ..)| *spec == report.spec)?;
        let (_, side, mark) = self.clipboard.pasted.remove(at);
        Some((side, mark))
    }

    /// 경로 복사 in `form`: the paths of the items of pane `side` (the selected rows, or the row
    /// the user put the cursor on) go on the clipboard as text. Nothing for a pane without such
    /// an item.
    pub(super) fn copy_paths(&mut self, side: usize, form: PathForm) -> Vec<Effect> {
        let paths: Vec<VPath> = self
            .targets(side)
            .into_iter()
            .filter(|path| path.as_local().is_some())
            .collect();
        if paths.is_empty() {
            return Vec::new();
        }
        vec![Effect::Clipboard(ClipboardWork::PutPaths { paths, form })]
    }

    /// 경로 복사 in `form` from a menu: the items of the active pane.
    pub fn copy_paths_as(&mut self, form: PathForm) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        self.copy_paths(self.active, form)
    }

    /// How putting the paths on the clipboard went: a notice says why it did not.
    pub fn paths_copied(&mut self, result: Result<(), ClipboardFailure>) -> Vec<Effect> {
        match result {
            Ok(()) => Vec::new(),
            Err(failure) => vec![Effect::Notice(words::clipboard_failed(failure).to_owned())],
        }
    }
}

/// `paths` as local addresses, and how many were left out: a path that is not one (relative,
/// or with a name the system cannot have), and a Windows device or volume path that stays one
/// once normalized (`\\?\GLOBALROOT\…`, `\\?\Volume{…}\…`), which names no folder of a drive
/// or a share.
fn local_paths(paths: &[PathBuf]) -> (Vec<VPath>, usize) {
    let local: Vec<VPath> = paths
        .iter()
        .filter(|path| path.is_absolute())
        .filter_map(|path| VPath::local(path).ok())
        .filter(|path| !path.as_local().is_some_and(is_device_path))
        .collect();
    let left_out = paths.len() - local.len();
    (local, left_out)
}
