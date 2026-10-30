//! The address bar of the panes (NAV-03). A child of [`state`](super), so these methods see the
//! fields of [`AppState`].
//!
//! - **The `›` after a piece of the path row** (the root's too, and one after the last piece)
//!   opens the list of the folders in that piece's folder, under it
//!   ([`AppState::open_subfolders`]). A worker lists them
//!   ([`Loader::list_names`](crate::loader::Loader::list_names): folders only, those the panes
//!   show, and the links that lead to folders, at most [`MAX_NAMES`] of them and the count of
//!   the rest), and the list scrolls. ↑↓ move its cursor a row, `PageUp` and `PageDown` a page,
//!   Home and End to either end, Enter goes to the cursor's folder and Esc closes it
//!   ([`AppState::subfolders_key`]: the controller hands it the keys of `keys::list_key` while
//!   the list shows); a click on a row goes to its folder. Any other command of the window
//!   closes the list ([`AppState::key`]), and so does a change of what it belongs to
//!   ([`AppState::drop_stale_address`]: the keys in the other pane, another tab or folder in
//!   its pane, another layout, a dialog over the window); the window closes it too when the
//!   keys leave the panes, at a press elsewhere, and at a new size of the window.
//! - **The completion list of the path field** ([`AppState::path_edited`]): as the text
//!   changes, a worker lists the folders in the folder that the text's last separator ends
//!   whose names start with what follows that separator. A new text drops the request before
//!   it; while the new one is out, the names already found in that folder that still fit keep
//!   showing. When the names found were all the folder has for a start of the name that the
//!   new text only lengthens, they are narrowed here and the folder is not listed again. ↑↓
//!   move the cursor ([`AppState::completion_step`]), Tab completes the text with the cursor's
//!   name, or the first one, and a separator, which lists that folder next
//!   ([`AppState::complete_path`]; a Tab before the first names come waits for them), Enter
//!   goes to the cursor's folder or else to the text ([`AppState::enter_path`]), and Esc
//!   closes the list ([`AppState::dismiss_completion`]). None of them acts while an input
//!   method composes (the field's guard, in `pane.slint`).
//! - **What a typed path names** ([`read_typed`]): a `file:` URL is read as the local path it
//!   names ([`read_file_url`]); other text has `~` and the environment's variables expanded
//!   first ([`expand_user_path`]). A remote address (`sftp://…`) gets the notice that the app
//!   cannot open it yet.
//!
//! A folder chosen in either list goes as a typed path goes: into the tab the pane shows, or
//! into a new tab beside a pinned one ([`AppState::navigate_shown`]); navigation sync leaves
//! the other pane where it is ([`layout`](super::layout)).

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fmt::Write as _;

use rf_core::{ErrorKind, Os, VPath};
use rf_vfs::VisibilityFilter;

use super::{AppState, Effect, Layout, load};
use crate::keys::ListKey;
use crate::loader::{FolderName, LoadEvent, NamesRequest, name_starts_with};
use crate::pane::{ancestors, next_generation};
use crate::words;

/// How many folder names a list of the address bar gets at most; it counts the others.
pub const MAX_NAMES: usize = 1000;

/// Where the variables of a typed path, and `~`, come from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Environment {
    /// The app's own environment (`std::env::var`; on Windows its names ignore case).
    #[default]
    Process,
    /// These variables alone, by name.
    Fixed(Vec<(String, String)>),
}

impl Environment {
    /// The value of the variable `name`; `None` when it is not set, or is not text.
    pub fn var(&self, name: &str) -> Option<String> {
        match self {
            Environment::Process => std::env::var(name).ok(),
            Environment::Fixed(vars) => vars
                .iter()
                .find(|(each, _)| each == name)
                .map(|(_, value)| value.clone()),
        }
    }
}

/// The names a list of the address bar shows, as far as a worker found them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Names {
    /// The worker is listing the folder.
    Loading,
    /// The folders, in the natural order of their names, and how many more there are than
    /// [`MAX_NAMES`].
    Listed { names: Vec<FolderName>, more: usize },
    /// The folder could not be listed.
    Failed(ErrorKind),
}

impl Names {
    /// The folders the list shows as rows: none until they are listed.
    pub fn rows(&self) -> &[FolderName] {
        match self {
            Names::Listed { names, .. } => names,
            Names::Loading | Names::Failed(_) => &[],
        }
    }

    /// What shows while a new listing with the name start `prefix` is out on `os`: the names
    /// found that still fit, or nothing until it comes.
    fn narrowed(self, prefix: &str, os: Os) -> Names {
        match self {
            Names::Listed { mut names, .. } => {
                names.retain(|each| name_starts_with(&each.name, prefix, os));
                Names::Listed { names, more: 0 }
            }
            Names::Loading | Names::Failed(_) => Names::Loading,
        }
    }
}

/// The list of the folders in a piece of a path row's folder, under its `›`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subfolders {
    side: usize,
    /// The piece of the path row (0 the root).
    index: usize,
    folder: VPath,
    names: Names,
    cursor: Option<usize>,
    generation: u64,
    /// What the list belongs to: the tab (its id) and the folder pane `side` showed, and the
    /// layout, when it opened. Once one of them changes, it closes.
    tab: u64,
    location: VPath,
    layout: Layout,
}

impl Subfolders {
    /// The pane whose path row it is under.
    pub fn side(&self) -> usize {
        self.side
    }

    /// The piece of the path row whose folder it lists: 0 for the root.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The folder it lists.
    pub fn folder(&self) -> &VPath {
        &self.folder
    }

    pub fn names(&self) -> &Names {
        &self.names
    }

    /// The row of the keyboard's cursor; none until a key moves it.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }
}

/// The completion list of the path field of a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completion {
    side: usize,
    /// The field's text it completes, as typed.
    text: String,
    /// That text as far as its last separator (included), and the folder it names.
    head: String,
    folder: VPath,
    /// The start of the names it lists: the text after `head`.
    prefix: String,
    names: Names,
    /// `names` are all the folder has that start with `prefix`: a listing for it, or for a
    /// shorter start, came whole (with no folder left out and no link left unexamined).
    complete: bool,
    /// A request for the names of this text is out: until it comes, a Tab with no row to
    /// complete with waits for it.
    waiting: bool,
    /// A Tab came while the list waited with no row: the text is completed once the names come.
    tab_waiting: bool,
    cursor: Option<usize>,
    generation: u64,
    /// The tab (its id) the pane showed, and the layout, when the field's text last changed.
    tab: u64,
    layout: Layout,
}

impl Completion {
    /// The pane whose path field it completes.
    pub fn side(&self) -> usize {
        self.side
    }

    /// The text it completes.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The folder whose folders it lists.
    pub fn folder(&self) -> &VPath {
        &self.folder
    }

    pub fn names(&self) -> &Names {
        &self.names
    }

    /// The row of the keyboard's cursor; none until ↑ or ↓.
    pub fn cursor(&self) -> Option<usize> {
        self.cursor
    }

    /// A request for the names of the text is out (a Tab with no row waits for it).
    pub fn waiting(&self) -> bool {
        self.waiting
    }

    /// The folder of the cursor's row.
    fn chosen(&self) -> Option<VPath> {
        let row = self.names.rows().get(self.cursor?)?;
        self.folder.join(&row.name).ok()
    }
}

/// What text typed into a path field names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    /// A local path, to go to.
    Folder(VPath),
    /// The address of a remote place ([`is_remote_url`]), which the app cannot open yet.
    Remote,
    /// No path the app can open: a relative one, or one that no system's names allow.
    Invalid,
}

/// `text` with `~` and the variables of the environment expanded by the rules of `os`; `var`
/// gives the value of a variable by its name.
///
/// - `~` on its own at the start, or followed by a separator, is the home folder: `HOME` on
///   Linux and macOS (as their shells have it), `USERPROFILE` on Windows (as PowerShell has
///   it; File Explorer has no `~`). `~user` is not looked up.
/// - Windows: `%NAME%`, the name being anything between two `%` (`%ProgramFiles(x86)%`).
/// - Linux and macOS: `$NAME` and `${NAME}`, a name being letters, digits and `_`, starting
///   with no digit; `$NAME` runs as far as such characters go.
///
/// A variable that is not set (or whose value is not text), and what is no variable, stays as
/// typed, as File Explorer leaves `%NOPE%` and unlike a shell, which drops an unknown `$NOPE`:
/// `$` and `%` may be part of a folder's name, and an unknown variable turned into nothing
/// would take the path somewhere else. A value is not expanded again.
pub fn expand_user_path(text: &str, os: Os, var: &dyn Fn(&str) -> Option<String>) -> String {
    let mut expanded = String::with_capacity(text.len());
    let mut rest = text;
    if let Some(after) = text.strip_prefix('~')
        && (after.is_empty() || after.starts_with(|c| is_separator(c, os)))
        && let Some(home) = var(if os == Os::Windows {
            "USERPROFILE"
        } else {
            "HOME"
        })
    {
        expanded.push_str(&home);
        rest = after;
    }
    if os == Os::Windows {
        expand_percent(rest, var, &mut expanded);
    } else {
        expand_dollar(rest, var, &mut expanded);
    }
    expanded
}

/// Whether `c` separates the names of a path typed on `os`: `/`, and on Windows `\` too.
fn is_separator(c: char, os: Os) -> bool {
    c == '/' || (os == Os::Windows && c == '\\')
}

/// Appends `text` to `out` with each `%NAME%` that `var` knows replaced by its value. An
/// unknown one keeps both of its `%`, and the text goes on after the second; a `%` with no
/// other after it stays.
fn expand_percent(text: &str, var: &dyn Fn(&str) -> Option<String>, out: &mut String) {
    let mut rest = text;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('%') else {
            out.push_str(&rest[open..]);
            return;
        };
        let name = &after[..close];
        match Some(name).filter(|name| !name.is_empty()).and_then(var) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[open..open + close + 2]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
}

/// Appends `text` to `out` with each `$NAME` and `${NAME}` that `var` knows replaced by its
/// value; an unknown one, and a `$` that starts no name, stay as they are.
fn expand_dollar(text: &str, var: &dyn Fn(&str) -> Option<String>, out: &mut String) {
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, taken) = dollar_name(after);
        match Some(name).filter(|name| !name.is_empty()).and_then(var) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[at..=at + taken]),
        }
        rest = &after[taken..];
    }
    out.push_str(rest);
}

/// The name of the variable that `after`, the text after a `$`, starts with (`NAME` or
/// `{NAME}`), and how much of `after` the variable takes; an empty name, taking nothing, when it
/// starts none.
fn dollar_name(after: &str) -> (&str, usize) {
    if let Some(braced) = after.strip_prefix('{') {
        return match braced.find('}') {
            Some(end) if is_variable_name(&braced[..end]) => (&braced[..end], end + 2),
            _ => ("", 0),
        };
    }
    let end = after
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(after.len());
    if is_variable_name(&after[..end]) {
        (&after[..end], end)
    } else {
        ("", 0)
    }
}

/// Whether `name` can be the name of a variable in `$NAME`: letters, digits and `_`, starting
/// with no digit.
fn is_variable_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Whether `text` is the address of a remote place given by a scheme and `//`
/// (`sftp://host/dir`, `smb://server/share`, `https://…`): the app opens the paths of the local
/// file systems alone for now (a Windows share as `\\server\share` is one of those). A drive
/// (`C://`) is no scheme; `local:`, the app's own form of a local path, and `file:`, which
/// [`read_file_url`] reads, are no remote addresses.
pub fn is_remote_url(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    scheme.len() >= 2
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        && !scheme.eq_ignore_ascii_case("local")
        && !scheme.eq_ignore_ascii_case("file")
}

/// What a `file:` URL names on a system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileUrl {
    /// This path of the system.
    Path(String),
    /// A file on another computer, which the system reaches by no local path (Linux, macOS).
    Remote,
    /// No path: one that is not text once decoded, or a Windows one without a drive or a
    /// computer.
    Invalid,
}

/// What `text` names on `os` when it is a `file:` URL (`file:///C:/x`, `file://server/share`,
/// `file:/x`; the scheme in any case); `None` for other text.
///
/// The path is percent-decoded (a `%` that starts no `%XX` stays as it is) and cut at a `?` or
/// `#`. On Windows `file:///C:/x` (and `file:///C|/x`) is `C:\x`, and a computer's name, given
/// as `file://server/share/x` or `file:////server/share/x`, makes the share path
/// `\\server\share\x`. On Linux and macOS the computer must be none or `localhost`:
/// `file:///x` and `file://localhost/x` are `/x`, and any other computer is remote.
pub fn read_file_url(text: &str, os: Os) -> Option<FileUrl> {
    let scheme = text.get(..5)?;
    if !scheme.eq_ignore_ascii_case("file:") {
        return None;
    }
    let rest = &text[5..];
    let (host, path) = match rest.strip_prefix("//") {
        Some(after) => after.split_at(after.find('/').unwrap_or(after.len())),
        None if rest.starts_with('/') => ("", rest),
        None => return Some(FileUrl::Invalid),
    };
    let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
    let (Some(host), Some(path)) = (percent_decode(host, os), percent_decode(path, os)) else {
        return Some(FileUrl::Invalid);
    };
    let here = host.is_empty() || host.eq_ignore_ascii_case("localhost");
    if os != Os::Windows {
        return Some(if here {
            FileUrl::Path(if path.is_empty() {
                "/".to_owned()
            } else {
                path
            })
        } else {
            FileUrl::Remote
        });
    }
    let windows = |path: &str| path.replace('/', "\\");
    if !here {
        return Some(FileUrl::Path(format!("\\\\{host}{}", windows(&path))));
    }
    if let Some(share) = path.strip_prefix("//") {
        return Some(FileUrl::Path(format!("\\\\{}", windows(share))));
    }
    // `/C:/x` or `/C|/x`: the drive, and the path on it from its root.
    let drive = path.strip_prefix('/').and_then(|after| {
        let mut chars = after.chars();
        let (drive, mark) = (chars.next()?, chars.next()?);
        (drive.is_ascii_alphabetic() && matches!(mark, ':' | '|'))
            .then_some((drive, chars.as_str()))
    });
    Some(match drive {
        Some((drive, "")) => FileUrl::Path(format!("{drive}:\\")),
        Some((drive, on_drive)) if on_drive.starts_with('/') => {
            FileUrl::Path(format!("{drive}:{}", windows(on_drive)))
        }
        _ => FileUrl::Invalid,
    })
}

/// The folder name `name` as a name of a `file:` URL: `%`, `#`, `?`, spaces and control
/// characters percent-encoded (each byte of their UTF-8), so that [`read_file_url`] reads it back
/// as it is; other text, letters of any script among it, stays as it is.
pub(super) fn encode_url_name(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for c in name.chars() {
        if matches!(c, '%' | '#' | '?' | ' ') || c.is_control() {
            let mut bytes = [0; 4];
            for byte in c.encode_utf8(&mut bytes).bytes() {
                let _ = write!(encoded, "%{byte:02X}");
            }
        } else {
            encoded.push(c);
        }
    }
    encoded
}

/// `text` with each `%XX` turned into its byte; `None` when the bytes are not UTF-8, or one of
/// them is a path separator of `os` (`%2F`, and `%5C` on Windows): an encoded separator would
/// make a name two, so it is no path, as browsers have it. On Linux and macOS a backslash is a
/// character of a name like any other. A `%` that two hexadecimal digits do not follow stays as
/// it is.
fn percent_decode(text: &str, os: Os) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |offset: usize| {
            bytes
                .get(at + offset)
                .and_then(|&digit| char::from(digit).to_digit(16))
        };
        match (bytes[at], hex(1), hex(2)) {
            (b'%', Some(high), Some(low)) => {
                let byte = u8::try_from(high * 16 + low).ok()?;
                if byte == b'/' || (byte == b'\\' && os == Os::Windows) {
                    return None;
                }
                decoded.push(byte);
                at += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// What `text`, typed into a path field, names: it is trimmed, then a `file:` URL is read as
/// the path it names on `os` ([`read_file_url`], before anything is expanded, so that its `%20`
/// is no `%VAR%`), and other text is expanded by the rules of `os` ([`expand_user_path`]). The
/// path is read as one of this system (`VPath::parse`).
pub fn read_typed(text: &str, os: Os, var: &dyn Fn(&str) -> Option<String>) -> Typed {
    let text = text.trim();
    let path = match read_file_url(text, os) {
        Some(FileUrl::Path(path)) => path,
        Some(FileUrl::Remote) => return Typed::Remote,
        Some(FileUrl::Invalid) => return Typed::Invalid,
        None => expand_user_path(text, os, var),
    };
    if is_remote_url(&path) {
        return Typed::Remote;
    }
    match VPath::parse(&path) {
        Ok(path) => Typed::Folder(path),
        Err(_) => Typed::Invalid,
    }
}

/// `text` split after its last separator on `os` (the separator stays with the first part):
/// the folder typed so far, and the start of a name in it. `None` without a separator.
pub fn split_last(text: &str, os: Os) -> Option<(&str, &str)> {
    let at = text.rfind(|c| is_separator(c, os))?;
    Some(text.split_at(at + 1))
}

/// Where the completion of `text` looks on `os`: `text` as far as its last separator, the
/// folder that names ([`read_typed`]), and the start of the name after it (percent-decoded in
/// a `file:` URL). `None` when that part names no folder the app can open.
fn completion_base<'t>(
    text: &'t str,
    os: Os,
    var: &dyn Fn(&str) -> Option<String>,
) -> Option<(&'t str, Cow<'t, str>, VPath)> {
    let (head, prefix) = split_last(text, os)?;
    let Typed::Folder(folder) = read_typed(head, os, var) else {
        return None;
    };
    let prefix = if read_file_url(head.trim_start(), os).is_some() {
        Cow::Owned(percent_decode(prefix, os)?)
    } else {
        Cow::Borrowed(prefix)
    };
    Some((head, prefix, folder))
}

/// The cursor of a list of `count` rows moved `delta` rows from `cursor`: from no row, a step
/// down lands on the first row and a step up on the last (a page goes on from there), and it
/// stops at either end. `None` for a list with no row.
fn step(cursor: Option<usize>, delta: isize, count: usize) -> Option<usize> {
    let last = count.checked_sub(1)?;
    let row = match cursor {
        None if delta < 0 => last.saturating_add_signed(delta + 1),
        None => 0usize.saturating_add_signed(delta - 1),
        Some(row) => row.saturating_add_signed(delta),
    };
    Some(row.min(last))
}

impl AppState {
    /// Takes `~` and the variables of typed paths from `environment` from now on.
    pub fn set_environment(&mut self, environment: Environment) {
        self.environment = environment;
    }

    /// The list of subfolders under a `›`, when one shows.
    pub fn subfolders(&self) -> Option<&Subfolders> {
        self.subfolders.as_ref()
    }

    /// A click on the `›` after piece `index` of the path row of pane `side` (0 the root):
    /// the pane gets the keyboard, and the list of the folders in that piece's folder opens in
    /// place of any other, listed by a worker. Nothing opens under a dialog.
    pub fn open_subfolders(&mut self, side: usize, index: usize) -> Vec<Effect> {
        self.subfolders = None;
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        self.activate(side);
        self.completion = None;
        let tab = self.tab(side);
        let (id, location) = (tab.id(), tab.location().clone());
        let Some(folder) = ancestors(&location).into_iter().nth(index) else {
            return Vec::new();
        };
        let generation = next_generation();
        let request = self.names_request(generation, folder.clone(), "");
        self.subfolders = Some(Subfolders {
            side,
            index,
            folder,
            names: Names::Loading,
            cursor: None,
            generation,
            tab: id,
            location,
            layout: self.layout,
        });
        vec![Effect::ListNames(request)]
    }

    /// A key of the list of subfolders (`keys::list_key`): ↑↓ move its cursor a row, `PageUp`
    /// and `PageDown` `page` rows (the rows it shows), Home and End to the first and the last row;
    /// Enter goes to the cursor's folder (nothing without a cursor), Esc closes it.
    pub fn subfolders_key(&mut self, key: ListKey, page: usize) -> Vec<Effect> {
        let Some(list) = self.subfolders.as_mut() else {
            return Vec::new();
        };
        let count = list.names.rows().len();
        let page = isize::try_from(page.max(1)).unwrap_or(isize::MAX);
        match key {
            ListKey::Up => list.cursor = step(list.cursor, -1, count),
            ListKey::Down => list.cursor = step(list.cursor, 1, count),
            ListKey::PageUp => list.cursor = step(list.cursor, -page, count),
            ListKey::PageDown => list.cursor = step(list.cursor, page, count),
            ListKey::First => list.cursor = (count > 0).then_some(0),
            ListKey::Last => list.cursor = count.checked_sub(1),
            ListKey::Choose => {
                if let Some(row) = list.cursor {
                    return self.choose_subfolder(row);
                }
            }
            ListKey::Close => self.subfolders = None,
        }
        Vec::new()
    }

    /// Row `row` of the list of subfolders was chosen (Enter, or a click): the list closes,
    /// and its pane goes to that folder as to a typed path.
    pub fn choose_subfolder(&mut self, row: usize) -> Vec<Effect> {
        let Some(list) = self.subfolders.take() else {
            return Vec::new();
        };
        let Some(folder) = list
            .names
            .rows()
            .get(row)
            .and_then(|each| list.folder.join(&each.name).ok())
        else {
            return Vec::new();
        };
        self.activate(list.side);
        load(self.navigate_shown(list.side, folder))
    }

    /// The list of subfolders closes.
    pub fn close_subfolders(&mut self) {
        self.subfolders = None;
    }

    /// The completion list of the path field of pane `side`, when it has one.
    pub fn completion(&self, side: usize) -> Option<&Completion> {
        self.completion
            .as_ref()
            .filter(|completion| completion.side == side)
    }

    /// The text of the path field of pane `side` changed to `text`: a worker lists the folders
    /// in the folder that the text's last separator ends whose names start with what follows
    /// it, and the request before is dropped. While it lists, the names found before in the
    /// same folder that still fit show. When those names were all the folder has for a start
    /// that the new one only lengthens, they are all there is: no listing. Text without a
    /// separator, or whose part before it names no folder (a relative path, a remote address),
    /// has no completion.
    pub fn path_edited(&mut self, side: usize, text: &str) -> Vec<Effect> {
        self.activate(side);
        let before = self.completion.take().filter(|before| before.side == side);
        let environment = &self.environment;
        let Some((head, prefix, folder)) =
            completion_base(text, Os::CURRENT, &|name| environment.var(name))
        else {
            return Vec::new();
        };
        let (names, complete, kept) = match before {
            Some(before) if before.folder == folder => {
                let whole = before.complete
                    && name_starts_with(OsStr::new(prefix.as_ref()), &before.prefix, Os::CURRENT);
                let names = before.names.narrowed(&prefix, Os::CURRENT);
                (names, whole, whole.then_some(before.generation))
            }
            _ => (Names::Loading, false, None),
        };
        // A listing that came whole stays the list's.
        let (generation, effects) = if let Some(generation) = kept {
            (generation, Vec::new())
        } else {
            let generation = next_generation();
            let request = self.names_request(generation, folder.clone(), &prefix);
            (generation, vec![Effect::ListNames(request)])
        };
        self.completion = Some(Completion {
            side,
            text: text.to_owned(),
            head: head.to_owned(),
            folder,
            prefix: prefix.into_owned(),
            names,
            complete,
            waiting: kept.is_none(),
            tab_waiting: false,
            cursor: None,
            generation,
            tab: self.tab(side).id(),
            layout: self.layout,
        });
        effects
    }

    /// ↑ (`delta` below 0) or ↓ in the path field of pane `side` while its completion list
    /// shows: the cursor moves a row, from no row ↓ to the first one and ↑ to the last.
    pub fn completion_step(&mut self, side: usize, delta: isize) -> Vec<Effect> {
        if let Some(completion) = self.completion.as_mut().filter(|each| each.side == side) {
            let count = completion.names.rows().len();
            completion.cursor = step(completion.cursor, delta, count);
        }
        Vec::new()
    }

    /// Tab in the path field of pane `side`, which holds `text`: the text completed with the
    /// name of the cursor's row (the first row's without a cursor) and the separator the text
    /// used last, and the request for that folder's completions; `None`, and nothing done, when
    /// the list is not for `text` or has no row. With no row while a request for the text is
    /// out, the Tab waits for its names: the field gets the completed text when they come
    /// ([`Effect::FillPath`]). In a `file:` URL the name is percent-encoded (`%`, `#`, `?`,
    /// space and controls), so that the URL reads back as that folder.
    pub fn complete_path(&mut self, side: usize, text: &str) -> Option<(String, Vec<Effect>)> {
        let completion = self
            .completion
            .as_mut()
            .filter(|each| each.side == side && each.text == text)?;
        if completion.names.rows().is_empty() && completion.waiting {
            completion.tab_waiting = true;
            return None;
        }
        let row = completion
            .names
            .rows()
            .get(completion.cursor.unwrap_or(0))?;
        let separator = completion.head.chars().next_back()?;
        let name = row.name.to_string_lossy();
        let name = if read_file_url(completion.head.trim_start(), Os::CURRENT).is_some() {
            Cow::Owned(encode_url_name(&name))
        } else {
            name
        };
        let completed = format!("{}{name}{separator}", completion.head);
        let effects = self.path_edited(side, &completed);
        Some((completed, effects))
    }

    /// Row `row` of the completion list of pane `side` was clicked: the list closes, and the
    /// pane goes to that folder.
    pub fn choose_completion(&mut self, side: usize, row: usize) -> Vec<Effect> {
        let Some(completion) = self.completion.take_if(|each| each.side == side) else {
            return Vec::new();
        };
        let Some(folder) = completion
            .names
            .rows()
            .get(row)
            .and_then(|each| completion.folder.join(&each.name).ok())
        else {
            return Vec::new();
        };
        self.activate(side);
        load(self.navigate_shown(side, folder))
    }

    /// The completion list of pane `side` closes.
    pub fn dismiss_completion(&mut self, side: usize) {
        self.completion
            .take_if(|completion| completion.side == side);
    }

    /// Enter in the path field of pane `side`, which holds `text`: the pane goes to the folder
    /// of the completion list's cursor, when its list is for `text` and has one; or else to the
    /// path `text` names once `~` and the variables are expanded ([`read_typed`]), in the tab
    /// the pane shows or in a new one beside a pinned one. The completion list closes. A remote
    /// address, and text that is no path, give a notice and change nothing.
    pub fn enter_path(&mut self, side: usize, text: &str) -> Vec<Effect> {
        self.activate(side);
        let completion = self.completion.take_if(|each| each.side == side);
        let chosen = completion
            .filter(|each| each.text == text)
            .and_then(|each| each.chosen());
        let typed = match chosen {
            Some(folder) => Typed::Folder(folder),
            None if text.trim().is_empty() => return Vec::new(),
            None => {
                let environment = &self.environment;
                read_typed(text, Os::CURRENT, &|name| environment.var(name))
            }
        };
        match typed {
            Typed::Folder(folder) => load(self.navigate_shown(side, folder)),
            Typed::Remote => vec![Effect::Notice(words::remote_location().to_owned())],
            Typed::Invalid => vec![Effect::Notice(words::cannot_open(ErrorKind::InvalidPath))],
        }
    }

    /// Drops a list of the address bar that what it belongs to has left: the list of
    /// subfolders when its pane no longer has the keys, shows another tab or folder, the
    /// layout changed or a dialog shows over the window (or the app ends); the completion list
    /// when its pane no longer has the keys, shows another tab, the layout changed or a dialog
    /// shows. The controller calls this after every change.
    pub fn drop_stale_address(&mut self) {
        let quiet = self.modal.is_none() && !self.quitting;
        let shows = |state: &AppState, side: usize, tab: u64| {
            quiet && side == state.active && state.tab(side).id() == tab
        };
        if let Some(list) = &self.subfolders {
            let kept = shows(self, list.side, list.tab)
                && *self.tab(list.side).location() == list.location
                && self.layout == list.layout;
            if !kept {
                self.subfolders = None;
            }
        }
        if let Some(completion) = &self.completion
            && !(shows(self, completion.side, completion.tab) && self.layout == completion.layout)
        {
            self.completion = None;
        }
    }

    /// The generations of the listings the address bar's lists wait for.
    pub(super) fn address_generations(&self) -> impl Iterator<Item = u64> + '_ {
        self.subfolders.iter().map(|list| list.generation).chain(
            self.completion
                .iter()
                .map(|completion| completion.generation),
        )
    }

    /// Takes `event`, when a list of the address bar waits for its listing: the names, or the
    /// failure, show there, and the cursor stays on the name it was on, if that is still
    /// listed; a Tab that waited for the completion list's names completes the field now. Gives
    /// `event` back (`Err`) when no list waits for it.
    pub(super) fn address_loaded(&mut self, event: LoadEvent) -> Result<Vec<Effect>, LoadEvent> {
        let generation = event.generation();
        if let Some(list) = self
            .subfolders
            .as_mut()
            .filter(|list| list.generation == generation)
        {
            take_names(&mut list.names, &mut list.cursor, event);
            return Ok(Vec::new());
        }
        let Some(completion) = self
            .completion
            .as_mut()
            .filter(|completion| completion.generation == generation)
        else {
            return Err(event);
        };
        completion.complete = matches!(
            event,
            LoadEvent::Names {
                more: 0,
                links_left: false,
                ..
            }
        );
        completion.waiting = false;
        take_names(&mut completion.names, &mut completion.cursor, event);
        if !std::mem::take(&mut completion.tab_waiting) {
            return Ok(Vec::new());
        }
        let (side, text) = (completion.side, completion.text.clone());
        Ok(match self.complete_path(side, &text) {
            Some((completed, mut effects)) => {
                effects.push(Effect::FillPath {
                    side,
                    text: completed,
                });
                effects
            }
            None => Vec::new(),
        })
    }

    /// The request for the names of the folders in `folder` that start with `prefix`, under
    /// `generation`: the hidden ones when the panes show them, at most [`MAX_NAMES`].
    fn names_request(&self, generation: u64, folder: VPath, prefix: &str) -> NamesRequest {
        NamesRequest {
            generation,
            folder,
            prefix: prefix.to_owned(),
            filter: VisibilityFilter {
                show_hidden: self.show_hidden,
                ..VisibilityFilter::default()
            },
            limit: MAX_NAMES,
            os: Os::CURRENT,
        }
    }
}

/// Puts what the listing `event` found into `names`, and keeps `cursor` on the folder it was on
/// when that is still listed.
fn take_names(names: &mut Names, cursor: &mut Option<usize>, event: LoadEvent) {
    let kept = cursor.and_then(|row| names.rows().get(row).map(|each| each.name.clone()));
    *names = match event {
        LoadEvent::Names { names, more, .. } => Names::Listed { names, more },
        LoadEvent::Failed { kind, .. } => Names::Failed(kind),
        // A listing of names sends nothing else.
        LoadEvent::Listed { .. }
        | LoadEvent::Changes { .. }
        | LoadEvent::Rescan { .. }
        | LoadEvent::Vanished { .. }
        | LoadEvent::Space { .. } => return,
    };
    *cursor = kept.and_then(|name| names.rows().iter().position(|each| each.name == name));
}
