//! The system's own menu of items (Windows' shell menu, OPS-05) and its picker of the ways to
//! share them (macOS, 공유…).
//!
//! The menu shows where the app's would (`rf_platform::shell::system_menu`):
//!
//! - **A right click** with Shift, or any while 보기 ▸ 설정 says to show Windows' menu first, on a
//!   row of a pane's list or on its part without rows ([`Controller::right_pressed`]): the
//!   window's winit hook sees the press before Slint does, asks the state ([`right_click`]), and
//!   keeps the press from Slint when the state shows Windows' menu, so that the app's menu does
//!   not show too. The place of the press is noted as the place of the menus of the lists
//!   (`ItemMenu.at`), where 더 많은 옵션 표시 of the app's menu shows Windows' menu later.
//! - **Shift+F10** (and the menu key with the setting): by the row the item menu opens by from the
//!   keys, or by the top of the list for the folder's own menu, as Slint places the app's menus
//!   (`item-menu-place`, `blank-menu-place`).
//! - **더 많은 옵션 표시** at the end of the app's menus: where that menu showed.
//!
//! It runs a loop of the system's own on the UI thread until it closes, in which the window's
//! drop target and the system's messages to the window may come back into the app. So it shows
//! on the next turn of the event loop ([`Controller::ask_system_menu`]), out of the callback that
//! asked, with what it is of copied out of the state and no borrow of it held; and only if, by
//! then, no dialog has come, the app does not end, its items are still the ones chosen, and the
//! left button is up ([`Controller::system_menu`]). No menu shows while the left button is down
//! ([`Controller::right_button_pressed`]). Once it has closed, the keys go back to the panes, as
//! after the app's menus, and what was chosen goes to the state ([`system_menu_chosen`]): the app
//! does its own commands there (이름 바꾸기, the trash, the clipboard) through its job engine and
//! undo.
//!
//! The picker of 공유… shows the same way, on the next turn of the event loop, where the item menu
//! showed ([`Controller::ask_share`]).
//!
//! [`right_click`]: crate::state::AppState::right_click
//! [`system_menu_chosen`]: crate::state::AppState::system_menu_chosen

use std::path::{Path, PathBuf};
use std::time::Duration;

use rf_core::{Error, Os, VPath};
use rf_platform::shell::{self, SystemMenuChoice, WindowPoint};
use slint::ComponentHandle;

use super::{Controller, side, side_number, with};
use crate::state::{MenuPlace, SystemMenuAsk};
use crate::words::{self, ShellAction};
use crate::{DropPlace, ItemMenu};

impl Controller {
    /// The right button went down in the window, at the pointer winit told last, with the left
    /// button up ([`Controller::right_button_pressed`]): where the system has a menu of its own
    /// and the press is on a pane's list (a row, or the part without rows), the place of the
    /// menus of the lists is noted there, and the state says whether the system's menu shows in
    /// the place of the app's
    /// ([`AppState::right_click`](crate::state::AppState::right_click)). `true` when it does:
    /// the press is then kept from Slint, whose menu would show too.
    pub(super) fn right_pressed(&self) -> bool {
        if !shell::SYSTEM_MENU {
            return false;
        }
        let (Some(window), Some(at)) = (self.window.upgrade(), self.pointer.get()) else {
            return false;
        };
        // Not while a dialog, quick look or a list of the window shows over the panes.
        let found = window.invoke_spot_at(at.x, at.y);
        if !found.found || found.place != DropPlace::List {
            return false;
        }
        let menu = window.global::<ItemMenu<'_>>();
        menu.set_at_x(at.x);
        menu.set_at_y(at.y);
        let row = usize::try_from(found.index).ok();
        let mut shown = false;
        self.update(|state| match state.right_click(side(found.side), row) {
            Some(effects) => {
                shown = true;
                effects
            }
            None => Vec::new(),
        });
        shown
    }

    /// Shows the system's menu of items that the state asked for on the next turn of the event
    /// loop ([`Controller::system_menu`]), at its place, read now: after the scroll to its row,
    /// which this turn's redraw has made.
    pub(super) fn ask_system_menu(&self, ask: SystemMenuAsk) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let at = match ask.place {
            MenuPlace::Row { side, row } => window
                .invoke_item_menu_place(side_number(side), i32::try_from(row).unwrap_or(i32::MAX)),
            MenuPlace::Folder(side) => window.invoke_blank_menu_place(side_number(side)),
            MenuPlace::Shown => {
                let menu = window.global::<ItemMenu<'_>>();
                slint::LogicalPosition::new(menu.get_at_x(), menu.get_at_y())
            }
        };
        let at = WindowPoint { x: at.x, y: at.y };
        slint::Timer::single_shot(Duration::ZERO, move || {
            with(|c| c.system_menu(&ask, at));
        });
    }

    /// The system's menu of `ask` at `at`, until it closes, with no borrow of the state held;
    /// then the keys go back to the panes, and what was chosen to the state. A menu that could
    /// not show says why.
    ///
    /// Asked a turn of the event loop before, it shows only while the state still has it due
    /// ([`AppState::system_menu_due`](crate::state::AppState::system_menu_due): no dialog came
    /// meanwhile, the app does not end, and its items are still the ones chosen), and while the
    /// left button is up: its loop would take the button's release and leave a drag running with
    /// no button held ([`Controller::right_button_pressed`]). Else nothing shows, and the keys
    /// stay where they are.
    fn system_menu(&self, ask: &SystemMenuAsk, at: WindowPoint) {
        let due = self
            .state
            .borrow_mut()
            .as_mut()
            .is_some_and(|state| state.system_menu_due(ask.token));
        if !due || self.left_down.get() {
            return;
        }
        let shown = match ask.folder.as_local() {
            Some(folder) => shell::system_menu(self.owner(), folder, &ask.names, at, ask.extended),
            None => Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        };
        if let Some(window) = self.window.upgrade() {
            window.invoke_menu_closed();
        }
        let choice = match shown {
            Ok(choice) => choice,
            Err(error) => {
                let kind = Error::io("show the system's menu", ask.folder.clone(), error).kind();
                self.notice(&words::shell_failed(
                    ShellAction::SystemMenu,
                    kind,
                    Os::CURRENT,
                ));
                SystemMenuChoice::Nothing
            }
        };
        self.update(|state| state.system_menu_chosen(ask.token, choice));
    }

    /// Shows the system's picker of the ways to share `paths` (macOS) on the next turn of the
    /// event loop, where the item menu showed.
    pub(super) fn ask_share(&self, paths: Vec<VPath>) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let menu = window.global::<ItemMenu<'_>>();
        let at = WindowPoint {
            x: menu.get_at_x(),
            y: menu.get_at_y(),
        };
        slint::Timer::single_shot(Duration::ZERO, move || {
            with(|c| c.share(&paths, at));
        });
    }

    /// The system's picker of the ways to share `paths` at `at`; one that could not show says
    /// why.
    fn share(&self, paths: &[VPath], at: WindowPoint) {
        let Some(first) = paths.first() else {
            return;
        };
        let local: Vec<PathBuf> = paths
            .iter()
            .filter_map(|path| path.as_local().map(Path::to_path_buf))
            .collect();
        if let Err(error) = shell::share(self.owner(), &local, at) {
            let kind = Error::io("share", first.clone(), error).kind();
            self.notice(&words::shell_failed(ShellAction::Share, kind, Os::CURRENT));
        }
    }
}
