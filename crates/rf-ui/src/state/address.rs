//! The address bar of the panes (NAV-03). A child of [`state`](super), so these methods see the
//! fields of [`AppState`].
//!
//! - **The `›` after a piece of the path row** (the root's too, and one after the last piece)
//!   opens the list of the folders in that piece's folder, under it
//!   ([`AppState::open_subfolders`]). A worker lists them
//!   ([`Loader::list_names`](crate::loader::Loader::list_names): folders only, those the panes
//!   show, at most [`MAX_NAMES`] of them and the count of the rest), and the list scrolls. ↑↓
//!   move its cursor, Enter goes to the cursor's folder and Esc closes it
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
//!   showing. ↑↓ move the cursor ([`AppState::completion_step`]), Tab completes the text with
//!   the cursor's name, or the first one, and a separator, which lists that folder next
//!   ([`AppState::complete_path`]), Enter goes to the cursor's folder or else to the text
//!   ([`AppState::enter_path`]), and Esc closes the list ([`AppState::dismiss_completion`]).
//!   None of them acts while an input method composes (the field's guard, in `pane.slint`).
//! - **What a typed path names** ([`read_typed`]): `~` and the environment's variables are
//!   expanded first ([`expand_user_path`]), and a remote address (`sftp://…`) gets the notice
//!   that the app cannot open it yet.
//!
//! A folder chosen in either list goes as a typed path goes: into the tab the pane shows, or
//! into a new tab beside a pinned one ([`AppState::navigate_shown`]); navigation sync leaves
//! the other pane where it is ([`layout`](super::layout)).

use std::ffi::OsString;

use rf_core::{ErrorKind, Os, VPath};
use rf_vfs::VisibilityFilter;

use super::{AppState, Effect, Layout, load};
use crate::keys::ListKey;
use crate::loader::{LoadEvent, NamesRequest, name_starts_with};
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
    /// The names, in the natural order, and how many more there are than [`MAX_NAMES`].
    Listed { names: Vec<OsString>, more: usize },
    /// The folder could not be listed.
    Failed(ErrorKind),
}

impl Names {
    /// The names the list shows as rows: none until they are listed.
    pub fn rows(&self) -> &[OsString] {
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
                names.retain(|name| name_starts_with(name, prefix, os));
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

    /// The row of the keyboard's cursor; none until ↑ or ↓.
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
    names: Names,
    cursor: Option<usize>,
    generation: u64,
    /// The tab (its id) the pane showed when the field's text last changed.
    tab: u64,
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

    /// The folder of the cursor's row.
    fn chosen(&self) -> Option<VPath> {
        let name = self.names.rows().get(self.cursor?)?;
        self.folder.join(name).ok()
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

/// Whether `text` is the address of a place given by a scheme and `//` (`sftp://host/dir`,
/// `smb://server/share`, `https://…`, `file:///…`): the app opens the paths of the local file
/// systems alone for now (a Windows share as `\\server\share` is one of those). A drive
/// (`C://`) is no scheme, and `local:`, the app's own form of a local path, is not an address.
pub fn is_remote_url(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    scheme.len() >= 2
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        && !scheme.eq_ignore_ascii_case("local")
}

/// What `text`, typed into a path field, names: it is trimmed and expanded by the rules of
/// `os` ([`expand_user_path`]), then read as a path of this system (`VPath::parse`).
pub fn read_typed(text: &str, os: Os, var: &dyn Fn(&str) -> Option<String>) -> Typed {
    let expanded = expand_user_path(text.trim(), os, var);
    if is_remote_url(&expanded) {
        return Typed::Remote;
    }
    match VPath::parse(&expanded) {
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
/// folder that names ([`read_typed`]), and the start of the name after it. `None` when that
/// part names no folder the app can open.
fn completion_base<'t>(
    text: &'t str,
    os: Os,
    var: &dyn Fn(&str) -> Option<String>,
) -> Option<(&'t str, &'t str, VPath)> {
    let (head, prefix) = split_last(text, os)?;
    match read_typed(head, os, var) {
        Typed::Folder(folder) => Some((head, prefix, folder)),
        Typed::Remote | Typed::Invalid => None,
    }
}

/// The cursor of a list of `count` rows moved `delta` rows from `cursor`: from no row, ↓ goes to
/// the first one and ↑ to the last; it stops at either end. `None` for a list with no row.
fn step(cursor: Option<usize>, delta: isize, count: usize) -> Option<usize> {
    let last = count.checked_sub(1)?;
    Some(match cursor {
        None if delta < 0 => last,
        None => 0,
        Some(row) => row.saturating_add_signed(delta).min(last),
    })
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

    /// A key of the list of subfolders (`keys::list_key`): ↑↓ move its cursor, Enter goes to
    /// the cursor's folder (nothing without a cursor), Esc closes it.
    pub fn subfolders_key(&mut self, key: ListKey) -> Vec<Effect> {
        let Some(list) = self.subfolders.as_mut() else {
            return Vec::new();
        };
        let count = list.names.rows().len();
        match key {
            ListKey::Up => list.cursor = step(list.cursor, -1, count),
            ListKey::Down => list.cursor = step(list.cursor, 1, count),
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
            .and_then(|name| list.folder.join(name).ok())
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
    /// same folder that still fit show. Text without a separator, or whose part before it names
    /// no folder (a relative path, a remote address), has no completion.
    pub fn path_edited(&mut self, side: usize, text: &str) -> Vec<Effect> {
        self.activate(side);
        let before = self.completion.take();
        let environment = &self.environment;
        let Some((head, prefix, folder)) =
            completion_base(text, Os::CURRENT, &|name| environment.var(name))
        else {
            return Vec::new();
        };
        let names = match before {
            Some(before) if before.side == side && before.folder == folder => {
                before.names.narrowed(prefix, Os::CURRENT)
            }
            _ => Names::Loading,
        };
        let generation = next_generation();
        let request = self.names_request(generation, folder.clone(), prefix);
        self.completion = Some(Completion {
            side,
            text: text.to_owned(),
            head: head.to_owned(),
            folder,
            names,
            cursor: None,
            generation,
            tab: self.tab(side).id(),
        });
        vec![Effect::ListNames(request)]
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
    /// the list is not for `text` or has no row.
    pub fn complete_path(&mut self, side: usize, text: &str) -> Option<(String, Vec<Effect>)> {
        let completion = self
            .completion
            .as_ref()
            .filter(|each| each.side == side && each.text == text)?;
        let name = completion
            .names
            .rows()
            .get(completion.cursor.unwrap_or(0))?;
        let separator = completion.head.chars().next_back()?;
        let completed = format!("{}{}{separator}", completion.head, name.to_string_lossy());
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
            .and_then(|name| completion.folder.join(name).ok())
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
    /// when its pane no longer has the keys, shows another tab, or a dialog shows. The
    /// controller calls this after every change.
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
            && !shows(self, completion.side, completion.tab)
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
    /// listed. Gives `event` back when no list waits for it.
    pub(super) fn address_loaded(&mut self, event: LoadEvent) -> Option<LoadEvent> {
        let generation = event.generation();
        let (names, cursor) = if let Some(list) = self
            .subfolders
            .as_mut()
            .filter(|list| list.generation == generation)
        {
            (&mut list.names, &mut list.cursor)
        } else if let Some(completion) = self
            .completion
            .as_mut()
            .filter(|completion| completion.generation == generation)
        {
            (&mut completion.names, &mut completion.cursor)
        } else {
            return Some(event);
        };
        let kept = cursor.and_then(|row| names.rows().get(row).cloned());
        *names = match event {
            LoadEvent::Names { names, more, .. } => Names::Listed { names, more },
            LoadEvent::Failed { kind, .. } => Names::Failed(kind),
            // A listing of names sends nothing else.
            LoadEvent::Listed { .. }
            | LoadEvent::Changes { .. }
            | LoadEvent::Rescan { .. }
            | LoadEvent::Vanished { .. }
            | LoadEvent::Space { .. } => return None,
        };
        *cursor = kept.and_then(|name| names.rows().iter().position(|row| *row == name));
        None
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
