//! The system's own ways to open an item (OPS-04, OPS-06's system window, OPS-11) and the item
//! menu from the keyboard. A child of [`state`](super), so these methods see the fields of
//! [`AppState`].
//!
//! The item menu (a right click on a row, or the menu key, Linux's Shift+F10 and macOS ⌃↩) offers
//! 연결 프로그램으로 열기… for one file, 관리자 권한으로 실행 for one program where the system
//! runs one as administrator (not for a batch file at a path `cmd.exe` would read as a
//! command), 터미널에서 열기 for one folder, all three for one link (the worker finds where it
//! leads), and OS 속성 창 열기 for one item
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
//!
//! The system's own menu of items (Windows' shell menu, OPS-05) shows in the place of the app's
//! for Shift+F10 ([`AppState::system_menu_key`]) and a right click with Shift
//! ([`AppState::right_click`]), as in File Explorer of Windows 11; for the menu key and any right
//! click too while 보기 ▸ 설정 says to show it first; and from 더 많은 옵션 표시 at the end of the
//! app's menus ([`AppState::more_options`]). The state copies out what it is of
//! ([`SystemMenuAsk`]: the folder, the names of the items or none for the folder's own menu, its
//! place), and the controller shows it on the next turn of the event loop with no borrow held.
//! What the user chose there comes back to [`AppState::system_menu_chosen`], with the ask's
//! token: the commands the app has itself (이름 바꾸기, the trash or a deletion for good, 잘라내기,
//! 복사, 붙여넣기) are the app's, through its job engine and undo, while the pane still shows the
//! menu's folder and its items are still the ones a command acts on; the system carried out the
//! rest. Where the system has no
//! such menu (macOS, Linux), the keys show the app's menu. macOS shares the items through its
//! own picker instead ([`AppState::share`], 공유… in the item menu).

use std::ffi::OsString;
use std::path::PathBuf;

use rf_core::{Entry, EntryKind, ErrorKind, Os, VPath};
use rf_platform::shell::{App, AppVerb, Elevation, SystemMenuChoice, can_run_as_admin, elevation};

use super::links::{LinkFacts, link_facts};
use super::{AppState, Effect, RIGHT};
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
    /// Open a terminal in the folder, or the folder a link there leads to.
    Terminal(VPath),
    /// Show the system's properties window of the item.
    SystemProperties(VPath),
}

impl ShellWork {
    /// What the work is, for the notice of its failure.
    pub fn action(&self) -> ShellAction {
        match self {
            ShellWork::OpenWith { .. } | ShellWork::OpenWithApp { .. } => ShellAction::OpenWith,
            ShellWork::RunAsAdmin(_) => ShellAction::RunAsAdmin,
            ShellWork::Terminal(_) => ShellAction::Terminal,
            ShellWork::SystemProperties(_) => ShellAction::SystemProperties,
        }
    }
}

/// A menu that no click opened ([`Effect::ShowMenu`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuShown {
    /// The item menu of pane `side` by row `row` (the menu key), offering what
    /// [`AppState::item_menu`] says.
    Item { side: usize, row: usize },
    /// The menu of the folder of pane `side`, by the top of its list (the menu key with nothing
    /// chosen).
    Folder(usize),
    /// The menu of the apps that can open a file, with these titles, where the item menu
    /// showed last; the one chosen goes to [`AppState::open_with_chosen`].
    Apps(Vec<String>),
    /// The menu of 놓을 때 묻기 where a drag was dropped: 여기로 이동 where the drag allows a move
    /// (`moves`), 여기로 복사 where it allows a copy (`copies`), 여기에 링크 만들기 where it
    /// allows a link (`links`), and 취소; what it chooses goes to
    /// [`AppState::drop_menu_chosen`] and [`AppState::drop_menu_link`].
    AskDrop {
        moves: bool,
        copies: bool,
        links: bool,
    },
    /// The menu of favorite `index` of the sidebar, by its row (the menu key on it): 열기, 반대
    /// 패널에 열기, 이름 바꾸기, 즐겨찾기에서 제거, 위로, 아래로; what it chooses goes to
    /// [`AppState::favorite_command`].
    Favorite { index: usize },
    /// The menu of the sidebar's empty part (the menu key on any other row): 현재 폴더를
    /// 즐겨찾기에 추가; what it chooses goes to [`AppState::favorites_blank_command`].
    FavoritesBlank,
}

/// Where the system's own menu of items shows ([`SystemMenuAsk`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MenuPlace {
    /// By row `row` of pane `side`, where the item menu shows from the keys (Shift+F10).
    Row { side: usize, row: usize },
    /// By the top of pane `side`'s list, where the folder's menu shows from the keys.
    Folder(usize),
    /// Where the last menu of a list showed (`ItemMenu.at`): where the right button went down,
    /// or where the app's menu showed whose 더 많은 옵션 표시 asks for it.
    Shown,
}

/// The system's own menu of items to show (Windows' shell menu, OPS-05), with what it is of,
/// copied out of the state: the controller shows it on the next turn of the event loop, with no
/// borrow of the state held (`rf_platform::shell::system_menu`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemMenuAsk {
    /// Which ask it is: what is chosen in the menu comes back with it
    /// ([`AppState::system_menu_chosen`]).
    pub token: u64,
    /// The folder the items are in, or whose own menu it is.
    pub folder: VPath,
    /// The names of the items in it; none for the folder's own menu (the part of a list without
    /// items).
    pub names: Vec<OsString>,
    pub place: MenuPlace,
    /// With the commands the system keeps for Shift (a right click or Shift+F10 with it).
    pub extended: bool,
}

/// The system's menu asked for last ([`AppState::system_menu_chosen`]), by its `token`: in pane
/// `side`, of the items `names` of `folder`, the items a command acts on there as it was asked,
/// or of the folder itself (no names).
#[derive(Clone, Debug, PartialEq, Eq)]
struct SystemAsked {
    token: u64,
    side: usize,
    folder: VPath,
    names: Vec<OsString>,
}

/// What the item menu offers for the items it is for, besides what it always has.
#[allow(
    clippy::struct_excessive_bools,
    reason = "five items of a menu, each shown or not"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MenuFacts {
    /// One file: 연결 프로그램으로 열기….
    pub one_file: bool,
    /// One program, where the system runs one as administrator: 관리자 권한으로 실행.
    pub runnable: bool,
    /// One folder: 터미널에서 열기.
    pub one_folder: bool,
    /// One item that is a folder itself (a link is not known to lead to one, and the listing
    /// does not tell): 즐겨찾기에 추가.
    pub one_dir: bool,
    /// One item: OS 속성 창 열기.
    pub one_item: bool,
}

/// What the item menu offers for `items`, on a system that runs a program as administrator or
/// not (`runs_as_admin`).
///
/// A link (a symlink, a junction) gets what a file and what a folder get: the listing does not
/// tell where it leads, and the UI thread does not look. The worker does, as the command
/// starts (`crate::open`): 연결 프로그램 and 관리자 권한으로 실행 go ahead for a link to a file,
/// 터미널에서 열기 for a link to a folder, and the other way round a notice says where the link
/// leads. Whether a link runs as administrator is told by its own name and path, which are what
/// the system, and `cmd.exe`, are given.
pub fn menu_facts(items: &[(VPath, &Entry)], runs_as_admin: bool) -> MenuFacts {
    let [(path, entry)] = items else {
        return MenuFacts::default();
    };
    let file = matches!(entry.kind, EntryKind::File | EntryKind::Symlink);
    MenuFacts {
        one_file: file,
        runnable: file && runs_as_admin && path.as_local().is_some_and(can_run_as_admin),
        one_folder: matches!(entry.kind, EntryKind::Dir | EntryKind::Symlink),
        one_dir: entry.kind == EntryKind::Dir,
        one_item: true,
    }
}

/// A request for the apps that can open `file`, by its `token`, and where it was asked: the
/// pane `side`, its tab `tab` and that tab's `folder`. Its answer shows only there.
#[derive(Debug)]
struct AskedApps {
    token: u64,
    file: VPath,
    side: usize,
    tab: u64,
    folder: VPath,
}

/// What the commands of this module keep between their request and its answer.
#[derive(Debug, Default)]
pub(super) struct ShellState {
    /// The facts of the item menu that opened last.
    menu: MenuFacts,
    /// What its 링크 만들기 ▸ offers (module `links`).
    pub(super) links: LinkFacts,
    /// The last request for 연결 프로그램.
    asked: Option<AskedApps>,
    /// The apps of the menu that shows them last, and their file.
    apps: Option<(VPath, Vec<App>)>,
    token: u64,
    /// Whether the system has a menu of its own of items (Windows): the controller says so.
    system_offered: bool,
    /// The system's menu asked for last, until what was chosen there comes back.
    system: Option<SystemAsked>,
}

impl AppState {
    /// What the item menu offers for the items it opened for last.
    pub fn item_menu(&self) -> MenuFacts {
        self.shell.menu
    }

    /// Whether a right click shows Windows' own menu of the items first (보기 ▸ 설정, OPS-05).
    pub fn system_menu_first(&self) -> bool {
        self.system_menu_first
    }

    pub fn set_system_menu_first(&mut self, on: bool) {
        self.system_menu_first = on;
    }

    /// Notes what the item menu of pane `side` offers, as it opens for the items a command acts
    /// on there.
    pub(super) fn note_item_menu(&mut self, side: usize) {
        let entries = self.target_entries(side);
        let menu = menu_facts(&entries, rf_platform::shell::RUNS_AS_ADMIN);
        let links = link_facts(
            Os::CURRENT,
            &entries,
            rf_platform::symlink_needs_privilege(),
        );
        self.shell.menu = menu;
        self.shell.links = links;
    }

    /// Whether the system has a menu of its own of items that the app shows (Windows,
    /// `rf_platform::shell::SYSTEM_MENU`): the controller tells the state as it starts.
    pub fn offer_system_menu(&mut self, offered: bool) {
        self.shell.system_offered = offered;
    }

    /// Shift+F10 (Windows): the system's own menu of the items a command acts on in pane
    /// `side`, by the row the item menu opens by from the keys, scrolled to, with the commands
    /// the system keeps for Shift when it is held (`extended`); with none, the folder's own.
    /// Where the system has no menu of its own, the app's menu ([`AppState::context_menu`]).
    pub(super) fn system_menu_key(&mut self, side: usize, extended: bool) -> Vec<Effect> {
        if !self.shell.system_offered {
            return self.app_menu(side);
        }
        match self.menu_row(side) {
            Some(row) => {
                let mut effects = vec![Effect::ScrollTo { side, row }];
                effects.extend(self.ask_system_menu(side, MenuPlace::Row { side, row }, extended));
                effects
            }
            None => self.ask_folder_menu(side, MenuPlace::Folder(side), extended),
        }
    }

    /// Whether the system's own menu shows in the place of the app's for a right click and the
    /// menu key: the system has one, and Shift is held (`shift`) or 보기 ▸ 설정 says to show it
    /// first.
    fn system_menu_now(&self, shift: bool) -> bool {
        self.shell.system_offered && (shift || self.system_menu_first)
    }

    /// The right button went down on row `row` of pane `side`'s list (`None`: on the part of
    /// it without rows), with the keys the window told last held. `Some`, with its effects, when
    /// the system's own menu shows there in the place of the app's: the system has one, and
    /// Shift is held or 보기 ▸ 설정 says to show it first, and no window or quick look shows
    /// over the main one. The row is chosen as a right click chooses it, and the pane gets the
    /// keys. `None` leaves the press to the app's own menus.
    pub fn right_click(&mut self, side: usize, row: Option<usize>) -> Option<Vec<Effect>> {
        let shift = self.drag_mods.shift;
        if self.modal.is_some()
            || self.quitting
            || self.quick_look_open()
            || !self.system_menu_now(shift)
        {
            return None;
        }
        let side = side.min(RIGHT);
        let mut effects = vec![Effect::FocusPanes];
        if let Some(row) = row {
            self.row_menu(side, row);
            effects.extend(self.ask_system_menu(side, MenuPlace::Shown, shift));
        } else {
            self.activate(side);
            effects.extend(self.ask_folder_menu(side, MenuPlace::Shown, shift));
        }
        Some(effects)
    }

    /// 더 많은 옵션 표시 at the end of the item menu, or of the menu of the part of a list without
    /// items (`blank`): the system's own menu of the same items (or folder), where that menu
    /// showed, in the active pane, which its choice has made active.
    pub fn more_options(&mut self, blank: bool) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting || !self.shell.system_offered {
            return Vec::new();
        }
        let side = self.active;
        if blank {
            self.ask_folder_menu(side, MenuPlace::Shown, false)
        } else {
            self.ask_system_menu(side, MenuPlace::Shown, false)
        }
    }

    /// The system's own menu of the items a command acts on in pane `side`, at `place`, with
    /// Shift's commands or not (`extended`): what it is of, copied out for the controller, which
    /// shows it; what is chosen there comes back to [`AppState::system_menu_chosen`] with its
    /// token. With no such item, the folder's own menu.
    fn ask_system_menu(&mut self, side: usize, place: MenuPlace, extended: bool) -> Vec<Effect> {
        let names = self.target_names(side);
        if names.is_empty() {
            return self.ask_folder_menu(side, place, extended);
        }
        self.system_menu_of(side, names, place, extended)
    }

    /// The system's own menu of the folder pane `side` shows, at `place` ([`ask_system_menu`]).
    ///
    /// [`ask_system_menu`]: AppState::ask_system_menu
    fn ask_folder_menu(&mut self, side: usize, place: MenuPlace, extended: bool) -> Vec<Effect> {
        self.system_menu_of(side, Vec::new(), place, extended)
    }

    /// The ask of the system's menu of the items `names` of pane `side`'s folder (none: the
    /// folder's own), noted under a new token for its answer.
    fn system_menu_of(
        &mut self,
        side: usize,
        names: Vec<OsString>,
        place: MenuPlace,
        extended: bool,
    ) -> Vec<Effect> {
        self.shell.token += 1;
        let token = self.shell.token;
        let folder = self.tab(side).location().clone();
        self.shell.system = Some(SystemAsked {
            token,
            side,
            folder: folder.clone(),
            names: names.clone(),
        });
        vec![Effect::SystemMenu(SystemMenuAsk {
            token,
            folder,
            names,
            place,
            extended,
        })]
    }

    /// The names of the items a command acts on in pane `side`, in the order of its rows.
    fn target_names(&self, side: usize) -> Vec<OsString> {
        self.target_entries(side)
            .iter()
            .map(|(_, entry)| entry.name.clone())
            .collect()
    }

    /// Whether pane `side` still shows the folder of the menu `asked`, and a command there still
    /// acts on the items it was of, whatever their order (none, for the folder's own menu).
    fn still_for(&self, asked: &SystemAsked) -> bool {
        if *self.tab(asked.side).location() != asked.folder {
            return false;
        }
        if asked.names.is_empty() {
            return true;
        }
        let mut now = self.target_names(asked.side);
        let mut then = asked.names.clone();
        now.sort_unstable();
        then.sort_unstable();
        now == then
    }

    /// Whether the system's menu of the ask `token` may show now: the controller shows it on
    /// the next turn of the event loop after the ask, and much may have come in between. It
    /// shows while it is the menu asked for last, no window or quick look shows over the main
    /// one, the app does not end, and its pane still shows its folder with the same items to act
    /// on (the items it was of are still there and chosen, as its choice needs them). Else it is
    /// forgotten and does not show: a dialog that came meanwhile (a job's question) stays in
    /// front, and a menu of other items than the ones chosen now would act on those.
    pub fn system_menu_due(&mut self, token: u64) -> bool {
        let Some(asked) = self
            .shell
            .system
            .as_ref()
            .filter(|asked| asked.token == token)
        else {
            return false;
        };
        let due = self.modal.is_none()
            && !self.quitting
            && !self.quick_look_open()
            && self.still_for(asked);
        if !due {
            self.shell.system = None;
        }
        due
    }

    /// What was chosen in the system's menu of the ask `token`, the one the state asked for
    /// last: a command the app does with its own (이름 바꾸기, the trash or a deletion for good,
    /// the clipboard's), for the items of that menu, as the same command of the app's menu does
    /// it, through the job engine and its undo; nothing for a command the system did itself,
    /// nothing chosen, or a choice no menu is waiting for (an earlier menu's, or a second one).
    ///
    /// The command acts on the items a command acts on now, so it is done only while they are
    /// still the menu's: the pane shows the same folder, and the same items are chosen (or, for
    /// the folder's own menu, any). Else a notice says that nothing was done: the selection or
    /// the folder changed between the ask and the answer (a key, a listing, a folder's watch).
    pub fn system_menu_chosen(&mut self, token: u64, choice: SystemMenuChoice) -> Vec<Effect> {
        let Some(asked) = self.shell.system.take_if(|asked| asked.token == token) else {
            return Vec::new();
        };
        let SystemMenuChoice::App(verb) = choice else {
            return Vec::new();
        };
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        if !self.still_for(&asked) {
            return vec![Effect::Notice(words::system_menu_moved().to_owned())];
        }
        let side = asked.side;
        match (verb, !asked.names.is_empty()) {
            // Into the folder of the menu's row, as the app's menu of a row pastes; the
            // folder's own menu pastes into the folder.
            (AppVerb::Paste, true) => {
                self.activate(side);
                self.paste_from_menu(false)
            }
            (AppVerb::Paste, false) => self.paste(side, false),
            (AppVerb::Rename, true) => self.rename(side),
            (AppVerb::Delete { permanent: false }, true) => self.trash(side),
            (AppVerb::Delete { permanent: true }, true) => self.delete_permanently(side),
            (AppVerb::Cut, true) => self.put_on_clipboard(side, true),
            (AppVerb::Copy, true) => self.put_on_clipboard(side, false),
            // A folder's own menu has none of the items' commands: none of them acts on the
            // items chosen in it.
            (AppVerb::Rename | AppVerb::Delete { .. } | AppVerb::Cut | AppVerb::Copy, false) => {
                Vec::new()
            }
        }
    }

    /// 공유… in the item menu (macOS): the system's picker of the ways to share the items a
    /// command acts on in the active pane, where the menu showed.
    pub fn share(&mut self) -> Vec<Effect> {
        if self.modal.is_some() || self.quitting {
            return Vec::new();
        }
        let paths: Vec<VPath> = self
            .targets(self.active)
            .into_iter()
            .filter(|path| path.as_local().is_some())
            .collect();
        if paths.is_empty() {
            return Vec::new();
        }
        vec![Effect::Share(paths)]
    }

    /// The menu key (Linux's Shift+F10, macOS ⌃↩): the menu of the items a command acts on in
    /// pane `side`, by the row of the cursor when it is one of them (else the first of them),
    /// scrolled to; with none, the menu of the pane's folder. Where 보기 ▸ 설정 says to show the
    /// system's own menu first (Windows), that one: by [`AppState::system_menu_key`], without
    /// Shift's commands.
    pub(super) fn context_menu(&mut self, side: usize) -> Vec<Effect> {
        if self.system_menu_now(false) {
            return self.system_menu_key(side, false);
        }
        self.app_menu(side)
    }

    /// The row the item menu of pane `side` opens by from the keys: the cursor's when it is one
    /// of the items a command acts on, else the first of them; `None` for none.
    fn menu_row(&self, side: usize) -> Option<usize> {
        let tab = self.tab(side);
        let chosen = tab.selection().indices(tab.visible());
        let cursor = tab
            .selection()
            .user_cursor()
            .filter(|&row| row < tab.visible().len());
        match (chosen.first(), cursor) {
            (None, cursor) => cursor,
            (Some(&first), Some(cursor)) => Some(if chosen.contains(&cursor) {
                cursor
            } else {
                first
            }),
            (Some(&first), None) => Some(first),
        }
    }

    /// The app's item menu of pane `side` from the keys ([`AppState::context_menu`]).
    fn app_menu(&mut self, side: usize) -> Vec<Effect> {
        let Some(row) = self.menu_row(side) else {
            self.shell.menu = MenuFacts::default();
            self.shell.links = LinkFacts::default();
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

    /// 연결 프로그램으로 열기…: the system's choice of the app for the one file (or link, which
    /// the worker finds leads to a file) chosen in pane `side`. Nothing for a folder, or for
    /// several items.
    pub(super) fn open_with(&mut self, side: usize) -> Vec<Effect> {
        let Some((file, EntryKind::File | EntryKind::Symlink)) = self.single_target(side) else {
            return Vec::new();
        };
        self.shell.token += 1;
        let token = self.shell.token;
        let tab = self.tab(side);
        self.shell.asked = Some(AskedApps {
            token,
            file: file.clone(),
            side,
            tab: tab.id(),
            folder: tab.location().clone(),
        });
        vec![Effect::Shell(ShellWork::OpenWith { token, file })]
    }

    /// The apps that can open the file of the request `token` (or why they are not known): a
    /// menu of them, the default one first, when it is the last request and the user is where
    /// they asked (no window and no quick look over the main one, and the same pane, tab,
    /// folder and file); else nothing: they have moved on.
    pub fn apps_found(&mut self, token: u64, found: Result<Vec<App>, ErrorKind>) -> Vec<Effect> {
        let Some(asked) = self.shell.asked.take_if(|asked| asked.token == token) else {
            return Vec::new();
        };
        if self.modal.is_some() || self.quitting || self.quick_look_open() || !self.still_at(&asked)
        {
            return Vec::new();
        }
        let file = asked.file;
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

    /// Whether the user is still where they asked for the apps of `asked`: its pane is the
    /// active one, on the same tab and folder, and its file is still the one item a command acts
    /// on there.
    fn still_at(&self, asked: &AskedApps) -> bool {
        let tab = self.tab(asked.side);
        self.active == asked.side
            && tab.id() == asked.tab
            && *tab.location() == asked.folder
            && self
                .single_target(asked.side)
                .is_some_and(|(path, _)| path == asked.file)
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

    /// 관리자 권한으로 실행: the one program (or link to one, which the worker checks) chosen in
    /// pane `side`, run as administrator. A batch file at a path that `cmd.exe` would read as a
    /// command is refused with a notice (the menu does not offer it: [`menu_facts`]).
    pub(super) fn run_as_admin(&mut self, side: usize) -> Vec<Effect> {
        let Some((file, EntryKind::File | EntryKind::Symlink)) = self.single_target(side) else {
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

    /// 터미널에서 열기 (Shift+F4): a terminal in the one folder chosen in pane `side` (or the
    /// folder a link chosen leads to, which the worker checks), or else in the folder the pane
    /// shows.
    pub(super) fn open_terminal(&mut self, side: usize) -> Vec<Effect> {
        let folder = match self.single_target(side) {
            Some((folder, EntryKind::Dir | EntryKind::Symlink)) => folder,
            _ => self.tab(side).location().clone(),
        };
        vec![Effect::Shell(ShellWork::Terminal(folder))]
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
