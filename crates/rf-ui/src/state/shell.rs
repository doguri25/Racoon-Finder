//! The system's own ways to open an item (OPS-04, OPS-06's system window, OPS-11) and the item
//! menu from the keyboard. A child of [`state`](super), so these methods see the fields of
//! [`AppState`].
//!
//! The item menu (a right click on a row, or Shift+F10, the menu key and macOS ⌃↩) offers
//! 연결 프로그램으로 열기… for one file, 관리자 권한으로 실행 for one program where the system
//! runs one as administrator (not for a batch file at a path `cmd.exe` would read as a
//! command), 터미널에서 열기 for one folder or link, and OS 속성 창 열기 for one item
//! ([`MenuFacts`], which the state keeps for the menu that opens last). The part of a list
//! without rows has 터미널에서 열기 too, for the pane's folder ([`AppState::folder_terminal`]),
//! and the properties window a button for the system's window of its one item
//! ([`AppState::system_properties_of_window`]).
//!
//! Each of them becomes an [`Effect::Shell`], which the controller carries out on a worker
//! thread (`crate::open`); a failure comes back as a notice. Where the system lists the apps
//! that can open a file rather than showing a dialog (macOS), the list comes back to
//! [`AppState::apps_found`], which shows it in a menu of its own ([`MenuShown::Apps`]) when
//! it is the answer to the last request and no window shows over the main one; the app chosen
//! there ([`AppState::open_with_chosen`]) opens the file.
//!
//! None of them changes a file: they act on the items a command acts on ([`AppState::targets`]:
//! the selection, or the row the user put the cursor on), as the item menu does.

use std::path::PathBuf;

use rf_core::{Entry, EntryKind, ErrorKind, Os, VPath};
use rf_platform::shell::{App, Elevation, can_run_as_admin, elevation};

use super::{AppState, Effect};
use crate::words::{self, ShellAction};

/// What a worker is to ask of the system (`crate::open::shell`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellWork {
    /// 연결 프로그램 for `file`: the system's dialog, or the apps that can open it, listed for
    /// [`AppState::apps_found`] with `token`, as the system does it
    /// (`rf_platform::shell::OPEN_WITH`).
    OpenWith { token: u64, file: VPath },
    /// Open `file` with the app at `app`, one the system listed.
    OpenWithApp { file: VPath, app: PathBuf },
    /// Run the program as administrator.
    RunAsAdmin(VPath),
    /// Open a terminal in the folder.
    Terminal(VPath),
    /// Open a terminal in the folder the link `link` leads to, found on the worker as it
    /// starts; in `otherwise` (the pane's folder) when it leads to no folder.
    LinkTerminal { link: VPath, otherwise: VPath },
    /// Show the system's properties window of the item.
    SystemProperties(VPath),
}

impl ShellWork {
    /// What the work is, for the notice of its failure.
    pub fn action(&self) -> ShellAction {
        match self {
            ShellWork::OpenWith { .. } | ShellWork::OpenWithApp { .. } => ShellAction::OpenWith,
            ShellWork::RunAsAdmin(_) => ShellAction::RunAsAdmin,
            ShellWork::Terminal(_) | ShellWork::LinkTerminal { .. } => ShellAction::Terminal,
            ShellWork::SystemProperties(_) => ShellAction::SystemProperties,
        }
    }
}

/// A menu that no click opened ([`Effect::ShowMenu`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuShown {
    /// The item menu of pane `side` by row `row` (Shift+F10), offering what
    /// [`AppState::item_menu`] says.
    Item { side: usize, row: usize },
    /// The menu of the folder of pane `side`, by the top of its list (Shift+F10 with nothing
    /// chosen).
    Folder(usize),
    /// The menu of the apps that can open a file, with these titles, where the item menu
    /// showed last; the one chosen goes to [`AppState::open_with_chosen`].
    Apps(Vec<String>),
}

/// What the item menu offers for the items it is for, besides what it always has.
#[allow(
    clippy::struct_excessive_bools,
    reason = "four items of a menu, each shown or not"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MenuFacts {
    /// One file: 연결 프로그램으로 열기….
    pub one_file: bool,
    /// One program, where the system runs one as administrator: 관리자 권한으로 실행.
    pub runnable: bool,
    /// One folder: 터미널에서 열기.
    pub one_folder: bool,
    /// One item: OS 속성 창 열기.
    pub one_item: bool,
}

/// What the item menu offers for `items`, on a system that runs a program as administrator or
/// not (`runs_as_admin`).
///
/// A link (a symlink, a junction) counts as a folder: the listing does not tell where it leads,
/// and 터미널에서 열기 opens the folder it leads to, or the pane's folder when it leads to a file
/// ([`ShellWork::LinkTerminal`]). It gets no 연결 프로그램, which is for a file.
pub fn menu_facts(items: &[(VPath, &Entry)], runs_as_admin: bool) -> MenuFacts {
    let [(path, entry)] = items else {
        return MenuFacts::default();
    };
    let file = entry.kind == EntryKind::File;
    MenuFacts {
        one_file: file,
        runnable: file && runs_as_admin && path.as_local().is_some_and(can_run_as_admin),
        one_folder: matches!(entry.kind, EntryKind::Dir | EntryKind::Symlink),
        one_item: true,
    }
}

/// What the commands of this module keep between their request and its answer.
#[derive(Debug, Default)]
pub(super) struct ShellState {
    /// The facts of the item menu that opened last.
    menu: MenuFacts,
    /// The last request for 연결 프로그램, by its token, and its file.
    asked: Option<(u64, VPath)>,
    /// The apps of the menu that shows them last, and their file.
    apps: Option<(VPath, Vec<App>)>,
    token: u64,
}

impl AppState {
    /// What the item menu offers for the items it opened for last.
    pub fn item_menu(&self) -> MenuFacts {
        self.shell.menu
    }

    /// Notes what the item menu of pane `side` offers, as it opens for the items a command acts
    /// on there.
    pub(super) fn note_item_menu(&mut self, side: usize) {
        self.shell.menu = menu_facts(
            &self.target_entries(side),
            rf_platform::shell::RUNS_AS_ADMIN,
        );
    }

    /// Shift+F10, the menu key (macOS ⌃↩): the menu of the items a command acts on in pane
    /// `side`, by the row of the cursor when it is one of them (else the first of them),
    /// scrolled to; with none, the menu of the pane's folder.
    pub(super) fn context_menu(&mut self, side: usize) -> Vec<Effect> {
        let tab = self.tab(side);
        let chosen = tab.selection().indices(tab.visible());
        let cursor = tab
            .selection()
            .user_cursor()
            .filter(|&row| row < tab.visible().len());
        let row = match (chosen.first(), cursor) {
            (None, cursor) => cursor,
            (Some(&first), Some(cursor)) => Some(if chosen.contains(&cursor) {
                cursor
            } else {
                first
            }),
            (Some(&first), None) => Some(first),
        };
        let Some(row) = row else {
            self.shell.menu = MenuFacts::default();
            return vec![Effect::ShowMenu(MenuShown::Folder(side))];
        };
        self.note_item_menu(side);
        vec![
            Effect::ScrollTo { side, row },
            Effect::ShowMenu(MenuShown::Item { side, row }),
        ]
    }

    /// The one item a command acts on in pane `side`, if there is exactly one.
    fn single_target(&self, side: usize) -> Option<(VPath, EntryKind)> {
        match self.target_entries(side).as_slice() {
            [(path, entry)] => Some((path.clone(), entry.kind)),
            _ => None,
        }
    }

    /// 연결 프로그램으로 열기…: the system's choice of the app for the one file chosen in pane
    /// `side`. Nothing for a folder, a link, or several items.
    pub(super) fn open_with(&mut self, side: usize) -> Vec<Effect> {
        let Some((file, EntryKind::File)) = self.single_target(side) else {
            return Vec::new();
        };
        self.shell.token += 1;
        let token = self.shell.token;
        self.shell.asked = Some((token, file.clone()));
        vec![Effect::Shell(ShellWork::OpenWith { token, file })]
    }

    /// The apps that can open the file of the request `token` (or why they are not known): a
    /// menu of them, the default one first, when it is the last request and no window shows
    /// over the main one; else nothing.
    pub fn apps_found(&mut self, token: u64, found: Result<Vec<App>, ErrorKind>) -> Vec<Effect> {
        let Some((_, file)) = self.shell.asked.take_if(|(asked, _)| *asked == token) else {
            return Vec::new();
        };
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        match found {
            Ok(apps) if apps.is_empty() => vec![Effect::Notice(words::no_apps().to_owned())],
            Ok(apps) => {
                let titles = apps.iter().map(words::app_title).collect();
                self.shell.apps = Some((file, apps));
                vec![Effect::ShowMenu(MenuShown::Apps(titles))]
            }
            Err(kind) => vec![Effect::Notice(words::shell_failed(
                ShellAction::OpenWith,
                kind,
                Os::CURRENT,
            ))],
        }
    }

    /// Item `index` of the menu of apps: the file opens with that app.
    pub fn open_with_chosen(&mut self, index: usize) -> Vec<Effect> {
        let Some((file, apps)) = self.shell.apps.take() else {
            return Vec::new();
        };
        apps.get(index)
            .map(|app| {
                Effect::Shell(ShellWork::OpenWithApp {
                    file,
                    app: app.path.clone(),
                })
            })
            .into_iter()
            .collect()
    }

    /// 관리자 권한으로 실행: the one program chosen in pane `side`, run as administrator.
    /// A batch file at a path that `cmd.exe` would read as a command is refused with a notice
    /// (the menu does not offer it: [`menu_facts`]).
    pub(super) fn run_as_admin(&mut self, side: usize) -> Vec<Effect> {
        let Some((file, EntryKind::File)) = self.single_target(side) else {
            return Vec::new();
        };
        match file.as_local().map(elevation) {
            Some(Elevation::Runs) => vec![Effect::Shell(ShellWork::RunAsAdmin(file))],
            Some(Elevation::UnsafeBatch) => {
                vec![Effect::Notice(words::batch_path_refused().to_owned())]
            }
            Some(Elevation::NotAProgram) | None => Vec::new(),
        }
    }

    /// 터미널에서 열기 (Shift+F4): a terminal in the one folder chosen in pane `side` (for a
    /// link, the folder it leads to, found on the worker), or else in the folder the pane
    /// shows.
    pub(super) fn open_terminal(&mut self, side: usize) -> Vec<Effect> {
        let here = self.tab(side).location().clone();
        let work = match self.single_target(side) {
            Some((folder, EntryKind::Dir)) => ShellWork::Terminal(folder),
            Some((link, EntryKind::Symlink)) => ShellWork::LinkTerminal {
                link,
                otherwise: here,
            },
            _ => ShellWork::Terminal(here),
        };
        vec![Effect::Shell(work)]
    }

    /// 터미널에서 열기 in the menu of the part of a list without rows: a terminal in the
    /// folder the active pane shows, whatever is chosen.
    pub fn folder_terminal(&mut self) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        let folder = self.tab(self.active).location().clone();
        vec![Effect::Shell(ShellWork::Terminal(folder))]
    }

    /// OS 속성 창 열기: the system's properties window of the one item chosen in pane `side`.
    pub(super) fn system_properties(&mut self, side: usize) -> Vec<Effect> {
        self.single_target(side)
            .map(|(path, _)| Effect::Shell(ShellWork::SystemProperties(path)))
            .into_iter()
            .collect()
    }

    /// OS 속성 창 열기 in the properties window: the system's window of its one item (the
    /// folder, when it is for the pane's folder) takes the place of the app's, which closes.
    /// Nothing for a window of several items.
    pub fn system_properties_of_window(&mut self) -> Vec<Effect> {
        let Some(path) = self
            .properties()
            .and_then(|properties| properties.single())
            .map(|item| item.path.clone())
        else {
            return Vec::new();
        };
        self.close();
        vec![Effect::Shell(ShellWork::SystemProperties(path))]
    }
}
