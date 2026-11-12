//! The item menu and the system's ways to open an item (OPS-04, OPS-06's system window,
//! OPS-11): what the menu offers for what is chosen, what each command asks of the system, the
//! menu of the apps that can open a file (macOS), and the item menu from the keyboard.

use std::ffi::OsString;
use std::path::PathBuf;

use rf_jobs::JobSpec;
use rf_platform::clipboard::DropEffect;
use rf_platform::dnd::DragMods;
use rf_platform::shell::{App, AppVerb, SystemMenuChoice};

use super::clipboard::{files, the_submit, the_work};
use super::*;
use crate::words::ShellAction;

fn in_a(name: &str) -> VPath {
    folder("a").join(name).expect("a plain name")
}

/// The left pane lists `docs`, `a.txt`, `b.txt` and `setup.exe`, in that order.
fn listed() -> AppState {
    let state = ready(
        vec![dir("docs"), file("a.txt"), file("b.txt"), file("setup.exe")],
        vec![],
    );
    assert_eq!(rows(&state, LEFT), ["docs", "a.txt", "b.txt", "setup.exe"]);
    state
}

/// The rows `chosen` of the left pane, selected.
fn choose(state: &mut AppState, chosen: &[usize]) {
    state.key(
        Command::ClearSelection,
        NONE,
        Paging::rows(10),
        Instant::now(),
    );
    for (i, &row) in chosen.iter().enumerate() {
        state.click_row(LEFT, row, i > 0, false);
    }
}

fn run(state: &mut AppState, command: Command) -> Vec<Effect> {
    state
        .key(command, NONE, Paging::rows(10), Instant::now())
        .expect("the window does it")
}

fn apps() -> Vec<App> {
    vec![
        App {
            name: "미리보기".to_owned(),
            path: PathBuf::from("/System/Applications/Preview.app"),
            default: true,
        },
        App {
            name: "TextEdit".to_owned(),
            path: PathBuf::from("/System/Applications/TextEdit.app"),
            default: false,
        },
    ]
}

/// 연결 프로그램 for the one file chosen: the token of its request.
fn ask_apps(state: &mut AppState) -> u64 {
    match run(state, Command::OpenWith).as_slice() {
        [Effect::Shell(ShellWork::OpenWith { token, .. })] => *token,
        other => panic!("one request, not {other:?}"),
    }
}

#[test]
fn the_item_menu_offers_what_the_items_take() {
    let (txt, exe, docs) = (file("a.txt"), file("setup.exe"), dir("docs"));
    let one = |path: &str, entry| [(in_a(path), entry)];
    assert_eq!(
        menu_facts(&one("a.txt", &txt), true),
        MenuFacts {
            one_file: true,
            runnable: false,
            one_folder: false,
            one_dir: false,
            one_item: true,
        }
    );
    // A program runs as administrator where the system does that.
    assert!(menu_facts(&one("setup.exe", &exe), true).runnable);
    assert!(!menu_facts(&one("setup.exe", &exe), false).runnable);
    assert_eq!(
        menu_facts(&one("docs", &docs), true),
        MenuFacts {
            one_folder: true,
            one_dir: true,
            one_item: true,
            ..MenuFacts::default()
        }
    );
    // Several items, or none: what is for one item alone does not show.
    let two = [(in_a("a.txt"), &txt), (in_a("docs"), &docs)];
    assert_eq!(menu_facts(&two, true), MenuFacts::default());
    assert_eq!(menu_facts(&[], true), MenuFacts::default());

    // A right click notes it for the menu that opens.
    let mut state = listed();
    state.row_menu(LEFT, 1);
    assert!(state.item_menu().one_file);
    state.row_menu(LEFT, 0);
    assert_eq!(state.item_menu(), menu_facts(&one("docs", &docs), false));
    // On a selected row, for the whole selection.
    choose(&mut state, &[1, 2]);
    state.row_menu(LEFT, 2);
    assert_eq!(state.item_menu(), MenuFacts::default());
}

#[test]
fn open_with_asks_the_system_for_the_one_file_chosen() {
    let mut state = listed();
    choose(&mut state, &[1]);
    assert_eq!(
        run(&mut state, Command::OpenWith),
        [Effect::Shell(ShellWork::OpenWith {
            token: 1,
            file: in_a("a.txt"),
        })]
    );
    // A folder, two files, nothing: no app to choose.
    for chosen in [&[0][..], &[1, 2], &[]] {
        choose(&mut state, chosen);
        assert_eq!(run(&mut state, Command::OpenWith), [], "{chosen:?}");
    }
}

#[test]
fn the_apps_of_the_last_request_show_and_the_one_chosen_opens_the_file() {
    let mut state = listed();
    choose(&mut state, &[2]);
    let first = ask_apps(&mut state);
    choose(&mut state, &[1]);
    let last = ask_apps(&mut state);
    // The first request's answer comes late: it shows nothing.
    assert_eq!(state.apps_found(first, Ok(apps())), []);
    assert_eq!(
        state.apps_found(last, Ok(apps())),
        [Effect::ShowMenu(MenuShown::Apps(vec![
            "미리보기 (기본)".to_owned(),
            "TextEdit".to_owned(),
        ]))]
    );
    // Once.
    assert_eq!(state.apps_found(last, Ok(apps())), []);
    assert_eq!(
        state.open_with_chosen(1),
        [Effect::Shell(ShellWork::OpenWithApp {
            file: in_a("a.txt"),
            app: PathBuf::from("/System/Applications/TextEdit.app"),
        })]
    );
    assert_eq!(state.open_with_chosen(0), []);
}

#[test]
fn no_app_or_a_failure_says_so_and_a_window_on_top_keeps_the_apps_away() {
    let mut state = listed();
    choose(&mut state, &[1]);
    let token = ask_apps(&mut state);
    assert_eq!(
        state.apps_found(token, Ok(Vec::new())),
        [Effect::Notice(words::no_apps().to_owned())]
    );
    let token = ask_apps(&mut state);
    assert_eq!(
        state.apps_found(token, Err(ErrorKind::PermissionDenied)),
        [Effect::Notice(words::shell_failed(
            ShellAction::OpenWith,
            ErrorKind::PermissionDenied,
            Os::CURRENT
        ))]
    );
    // The properties window opened meanwhile: no menu over it.
    let token = ask_apps(&mut state);
    run(&mut state, Command::Properties);
    assert!(state.modal().is_some());
    assert_eq!(state.apps_found(token, Ok(apps())), []);
    assert_eq!(state.open_with_chosen(0), []);
}

#[test]
fn the_apps_do_not_show_once_the_user_has_moved_on() {
    let mut state = listed();
    choose(&mut state, &[1]);
    // Quick look opened meanwhile.
    let token = ask_apps(&mut state);
    run(&mut state, Command::QuickLook);
    assert!(state.quick_look_open());
    assert_eq!(state.apps_found(token, Ok(apps())), []);
    run(&mut state, Command::QuickLook);
    // Another file chosen.
    let token = ask_apps(&mut state);
    choose(&mut state, &[2]);
    assert_eq!(state.apps_found(token, Ok(apps())), []);
    // The other pane took the keys.
    let token = ask_apps(&mut state);
    state.activate(RIGHT);
    assert_eq!(state.apps_found(token, Ok(apps())), []);
    state.activate(LEFT);
    // Another tab.
    let token = ask_apps(&mut state);
    run(&mut state, Command::NewTab);
    assert_eq!(state.apps_found(token, Ok(apps())), []);
    run(&mut state, Command::CloseTab);
    // Another folder.
    let token = ask_apps(&mut state);
    state.go_to(folder("b"));
    assert_eq!(state.apps_found(token, Ok(apps())), []);
    assert_eq!(state.open_with_chosen(0), []);
}

#[test]
fn a_terminal_opens_in_the_folder_chosen_or_else_in_the_pane_s() {
    let mut state = listed();
    let terminal = |state: &mut AppState| match run(state, Command::OpenTerminal).as_slice() {
        [Effect::Shell(ShellWork::Terminal(folder))] => folder.clone(),
        other => panic!("one terminal, not {other:?}"),
    };
    assert_eq!(terminal(&mut state), folder("a"));
    choose(&mut state, &[0]);
    assert_eq!(terminal(&mut state), in_a("docs"));
    // A file, or several items: the pane's folder.
    choose(&mut state, &[1]);
    assert_eq!(terminal(&mut state), folder("a"));
    choose(&mut state, &[0, 1]);
    assert_eq!(terminal(&mut state), folder("a"));
    // Shift+F4 is the key.
    choose(&mut state, &[0]);
    assert_eq!(
        press(&mut state, &special(Key::F4), SHIFT).expect("the terminal key"),
        [Effect::Shell(ShellWork::Terminal(in_a("docs")))]
    );
    // The menu of the part without rows: the pane's folder, whatever is chosen.
    assert_eq!(
        state.folder_terminal(),
        [Effect::Shell(ShellWork::Terminal(folder("a")))]
    );
}

#[test]
fn only_a_program_runs_as_administrator() {
    let mut state = listed();
    choose(&mut state, &[3]);
    assert_eq!(
        run(&mut state, Command::RunAsAdmin),
        [Effect::Shell(ShellWork::RunAsAdmin(in_a("setup.exe")))]
    );
    for chosen in [&[1][..], &[0], &[3, 1], &[]] {
        choose(&mut state, chosen);
        assert_eq!(run(&mut state, Command::RunAsAdmin), [], "{chosen:?}");
    }
}

#[test]
fn the_system_s_properties_window_is_for_one_item() {
    let mut state = listed();
    choose(&mut state, &[1]);
    assert_eq!(
        run(&mut state, Command::SystemProperties),
        [Effect::Shell(ShellWork::SystemProperties(in_a("a.txt")))]
    );
    for chosen in [&[1, 2][..], &[]] {
        choose(&mut state, chosen);
        assert_eq!(run(&mut state, Command::SystemProperties), [], "{chosen:?}");
    }
    // From the app's properties window: its one item, whose window closes for the system's.
    choose(&mut state, &[0]);
    run(&mut state, Command::Properties);
    assert_eq!(
        state.system_properties_of_window(),
        [Effect::Shell(ShellWork::SystemProperties(in_a("docs")))]
    );
    assert_eq!(state.modal(), None);
    // The window of the pane's own folder.
    choose(&mut state, &[]);
    run(&mut state, Command::Properties);
    assert_eq!(
        state.system_properties_of_window(),
        [Effect::Shell(ShellWork::SystemProperties(folder("a")))]
    );
    // A window of several items has none, and stays.
    choose(&mut state, &[1, 2]);
    run(&mut state, Command::Properties);
    assert_eq!(state.system_properties_of_window(), []);
    assert!(state.modal().is_some());
}

#[test]
fn the_keys_open_the_item_menu_by_the_cursor_or_the_folder_s_menu() {
    let mut state = listed();
    // Nothing chosen: the folder's menu.
    assert_eq!(
        press(&mut state, &special(Key::Menu), NONE).expect("the menu key"),
        [Effect::ShowMenu(MenuShown::Folder(LEFT))]
    );
    // Linux's Shift+F10 too (Windows' shows its own menu: below).
    let shift_f10 = keys::command_for(&special(Key::F10), SHIFT, Os::Other).expect("a key");
    assert_eq!(
        state.key(shift_f10, SHIFT, Paging::rows(10), Instant::now()),
        Some(vec![Effect::ShowMenu(MenuShown::Folder(LEFT))])
    );
    assert_eq!(state.item_menu(), MenuFacts::default());
    // The selection, by the row of the cursor among it.
    choose(&mut state, &[1, 2]);
    assert_eq!(
        press(&mut state, &special(Key::Menu), NONE).expect("the menu key"),
        [
            Effect::ScrollTo { side: LEFT, row: 2 },
            Effect::ShowMenu(MenuShown::Item { side: LEFT, row: 2 }),
        ]
    );
    // One file: the menu offers what is for it.
    choose(&mut state, &[3]);
    run(&mut state, Command::ContextMenu);
    assert!(state.item_menu().one_file);
    // The cursor moved off the selection (Ctrl+↑): by the first row chosen.
    choose(&mut state, &[3]);
    press(&mut state, &special(Key::UpArrow), CTRL);
    assert_eq!(
        run(&mut state, Command::ContextMenu),
        [
            Effect::ScrollTo { side: LEFT, row: 3 },
            Effect::ShowMenu(MenuShown::Item { side: LEFT, row: 3 }),
        ]
    );
    // While a window shows over the main one, nothing.
    run(&mut state, Command::Properties);
    assert_eq!(run(&mut state, Command::ContextMenu), []);
}

/// A folder that is absolute on every system, and whose path `cmd.exe` takes as it is: a
/// temporary folder could hold `(`, `&` or `%` (a user named "Kim (Dev)"), which would change
/// what runs as administrator. Nothing reads it: the state only names it.
fn tools() -> VPath {
    let path = if cfg!(windows) { r"C:\tools" } else { "/tools" };
    VPath::local(path).expect("an absolute path")
}

fn in_tools(name: &str) -> VPath {
    tools().join(name).expect("a plain name")
}

/// The left pane at [`tools`] lists a folder, links (one with a program's name), and batch
/// files (one at a path `cmd.exe` would read as a command); the row of each name.
fn with_links_and_scripts() -> (AppState, impl Fn(&AppState, &str) -> usize) {
    let (mut state, effects) = AppState::new(tools(), folder("b"), Some(folder("home")));
    for (request, entries) in loads(&effects).into_iter().zip([
        vec![
            dir("docs"),
            Entry::new("링크", EntryKind::Symlink),
            Entry::new("setup.exe", EntryKind::Symlink),
            file("build.bat"),
            file("R&D.bat"),
            file("메모.txt"),
        ],
        vec![],
    ]) {
        let generation = request.generation;
        state.loaded(LoadEvent::Listed {
            generation,
            entries,
        });
    }
    let row = |state: &AppState, name: &str| {
        rows(state, LEFT)
            .iter()
            .position(|row| *row == name)
            .expect("listed")
    };
    (state, row)
}

#[test]
fn a_link_gets_what_a_file_and_a_folder_get_and_the_worker_finds_which() {
    let (mut state, row) = with_links_and_scripts();
    let link = row(&state, "링크");
    // The listing does not tell where a link leads, and the UI thread does not look: the menu
    // offers what a file and a folder have, and the worker checks where it leads
    // (`open::link_leads`), with a notice when it leads elsewhere.
    state.row_menu(LEFT, link);
    assert_eq!(
        state.item_menu(),
        MenuFacts {
            one_file: true,
            runnable: false,
            // A link is not known to lead to a folder: it cannot be kept as a favorite.
            one_folder: true,
            one_dir: false,
            one_item: true,
        }
    );
    choose(&mut state, &[link]);
    assert_eq!(
        run(&mut state, Command::OpenWith),
        [Effect::Shell(ShellWork::OpenWith {
            token: 1,
            file: in_tools("링크"),
        })]
    );
    let terminal = [Effect::Shell(ShellWork::Terminal(in_tools("링크")))];
    assert_eq!(run(&mut state, Command::OpenTerminal), terminal);
    assert_eq!(
        press(&mut state, &special(Key::F4), SHIFT).expect("the terminal key"),
        terminal
    );
    // A link with a program's name runs as administrator by that name, which the system gets.
    let program = row(&state, "setup.exe");
    let entry = Entry::new("setup.exe", EntryKind::Symlink);
    assert!(menu_facts(&[(in_tools("setup.exe"), &entry)], true).runnable);
    assert!(!menu_facts(&[(in_tools("setup.exe"), &entry)], false).runnable);
    choose(&mut state, &[program]);
    assert_eq!(
        run(&mut state, Command::RunAsAdmin),
        [Effect::Shell(ShellWork::RunAsAdmin(in_tools("setup.exe")))]
    );
}

#[test]
fn a_batch_file_runs_as_administrator_only_at_a_path_cmd_takes_as_it_is() {
    let (mut state, row) = with_links_and_scripts();
    let build = row(&state, "build.bat");
    let unsafe_one = row(&state, "R&D.bat");
    // The menu offers it where the system runs a program as administrator (Windows).
    let entry = file("build.bat");
    assert!(menu_facts(&[(in_tools("build.bat"), &entry)], true).runnable);
    let entry = file("R&D.bat");
    assert!(!menu_facts(&[(in_tools("R&D.bat"), &entry)], true).runnable);
    choose(&mut state, &[build]);
    assert_eq!(
        run(&mut state, Command::RunAsAdmin),
        [Effect::Shell(ShellWork::RunAsAdmin(in_tools("build.bat")))]
    );
    // `cmd.exe /C` would run what follows the `&`, elevated: refused, and said why.
    choose(&mut state, &[unsafe_one]);
    assert_eq!(
        run(&mut state, Command::RunAsAdmin),
        [Effect::Notice(words::batch_path_refused().to_owned())]
    );
}

/// The left pane of [`listed`], where the system has a menu of its own (Windows), as the
/// controller tells the state there.
fn with_windows_menu() -> AppState {
    let mut state = listed();
    state.offer_system_menu(true);
    state
}

fn shift_held() -> DragMods {
    DragMods {
        shift: true,
        ..DragMods::default()
    }
}

/// The system's menu of the items `names` of the left pane's folder (none: the folder's own),
/// with no token: compare it with [`untokened`] effects.
fn system_menu(names: &[&str], place: MenuPlace, extended: bool) -> Effect {
    Effect::SystemMenu(SystemMenuAsk {
        token: 0,
        folder: folder("a"),
        names: names.iter().map(OsString::from).collect(),
        place,
        extended,
    })
}

/// `effects` with the token of each ask of the system's menu taken out, to compare with
/// [`system_menu`].
fn untokened(effects: Vec<Effect>) -> Vec<Effect> {
    effects
        .into_iter()
        .map(|effect| match effect {
            Effect::SystemMenu(ask) => Effect::SystemMenu(SystemMenuAsk { token: 0, ..ask }),
            other => other,
        })
        .collect()
}

/// The token of the ask of the system's menu that `effects` end with.
fn token_of(effects: &[Effect]) -> u64 {
    match effects.last() {
        Some(Effect::SystemMenu(ask)) => ask.token,
        _ => panic!("an ask of the system's menu, not {effects:?}"),
    }
}

#[test]
fn shift_f10_shows_windows_own_menu_of_the_items_or_of_the_folder() {
    let mut state = with_windows_menu();
    // Nothing chosen: the folder's own menu, by the top of the list, with Shift's commands.
    assert_eq!(
        untokened(press(&mut state, &special(Key::F10), SHIFT).expect("a key of the window")),
        [system_menu(&[], MenuPlace::Folder(LEFT), true)]
    );
    // The selection, by the row the item menu opens by, scrolled to.
    choose(&mut state, &[1, 2]);
    assert_eq!(
        untokened(press(&mut state, &special(Key::F10), SHIFT).expect("a key of the window")),
        [
            Effect::ScrollTo { side: LEFT, row: 2 },
            system_menu(
                &["a.txt", "b.txt"],
                MenuPlace::Row { side: LEFT, row: 2 },
                true
            ),
        ]
    );
    // The menu key shows the app's menu, which ends with 더 많은 옵션 표시.
    assert_eq!(
        press(&mut state, &special(Key::Menu), NONE).expect("a key of the window"),
        [
            Effect::ScrollTo { side: LEFT, row: 2 },
            Effect::ShowMenu(MenuShown::Item { side: LEFT, row: 2 }),
        ]
    );
    // In the right pane, of its own folder.
    state.activate(RIGHT);
    assert_eq!(
        untokened(run(&mut state, Command::SystemMenu)),
        [Effect::SystemMenu(SystemMenuAsk {
            token: 0,
            folder: folder("b"),
            names: Vec::new(),
            place: MenuPlace::Folder(RIGHT),
            extended: false,
        })]
    );
    // While a window shows over the main one, nothing.
    state.activate(LEFT);
    run(&mut state, Command::Properties);
    assert_eq!(run(&mut state, Command::SystemMenu), []);
    // Where the system has no menu of its own, the app's menu.
    let mut linux = listed();
    choose(&mut linux, &[1]);
    assert_eq!(
        run(&mut linux, Command::SystemMenu),
        [
            Effect::ScrollTo { side: LEFT, row: 1 },
            Effect::ShowMenu(MenuShown::Item { side: LEFT, row: 1 }),
        ]
    );
}

#[test]
fn a_right_click_with_shift_or_the_setting_shows_windows_own_menu() {
    let mut state = with_windows_menu();
    choose(&mut state, &[1]);
    // The setting off and no Shift: the app's menus take the click and the menu key.
    assert_eq!(state.right_click(LEFT, Some(2)), None);
    assert_eq!(state.right_click(LEFT, None), None);
    assert_eq!(
        run(&mut state, Command::ContextMenu),
        [
            Effect::ScrollTo { side: LEFT, row: 1 },
            Effect::ShowMenu(MenuShown::Item { side: LEFT, row: 1 }),
        ]
    );
    // Shift: Windows' own of the row clicked, which a right click chooses, where it was
    // clicked, with Shift's commands; the pane gets the keys.
    state.modifiers_changed(shift_held());
    assert_eq!(
        state.right_click(LEFT, Some(2)).map(untokened),
        Some(vec![
            Effect::FocusPanes,
            system_menu(&["b.txt"], MenuPlace::Shown, true),
        ])
    );
    assert_eq!(selected(&state, LEFT), [2]);
    // On the part of a list without rows: its folder's own, and the pane becomes active.
    assert_eq!(
        state.right_click(RIGHT, None).map(untokened),
        Some(vec![
            Effect::FocusPanes,
            Effect::SystemMenu(SystemMenuAsk {
                token: 0,
                folder: folder("b"),
                names: Vec::new(),
                place: MenuPlace::Shown,
                extended: true,
            }),
        ])
    );
    assert_eq!(state.active(), RIGHT);
    // The setting on: the menu key and a right click show Windows' own without Shift, and
    // without Shift's commands.
    state.modifiers_changed(DragMods::default());
    state.set_system_menu_first(true);
    assert_eq!(
        state.right_click(LEFT, Some(0)).map(untokened),
        Some(vec![
            Effect::FocusPanes,
            system_menu(&["docs"], MenuPlace::Shown, false),
        ])
    );
    assert_eq!(
        untokened(run(&mut state, Command::ContextMenu)),
        [
            Effect::ScrollTo { side: LEFT, row: 0 },
            system_menu(&["docs"], MenuPlace::Row { side: LEFT, row: 0 }, false),
        ]
    );
    // While a window or quick look shows over the main one, the click is left alone.
    run(&mut state, Command::Properties);
    assert_eq!(state.right_click(LEFT, Some(1)), None);
    // Where the system has no menu of its own, neither Shift nor the setting changes a thing
    // (a settings file from Windows).
    let mut linux = listed();
    linux.set_system_menu_first(true);
    linux.modifiers_changed(shift_held());
    assert_eq!(linux.right_click(LEFT, Some(1)), None);
    choose(&mut linux, &[1]);
    assert_eq!(
        run(&mut linux, Command::ContextMenu),
        [
            Effect::ScrollTo { side: LEFT, row: 1 },
            Effect::ShowMenu(MenuShown::Item { side: LEFT, row: 1 }),
        ]
    );
}

#[test]
fn more_options_shows_windows_own_menu_of_what_the_app_s_menu_was_for() {
    let mut state = with_windows_menu();
    state.row_menu(LEFT, 1);
    assert_eq!(
        untokened(state.more_options(false)),
        [system_menu(&["a.txt"], MenuPlace::Shown, false)]
    );
    // The part of the list without items: the folder's own.
    assert_eq!(
        untokened(state.more_options(true)),
        [system_menu(&[], MenuPlace::Shown, false)]
    );
    // Nothing where the system has no menu of its own, or while a window shows.
    let mut linux = listed();
    linux.row_menu(LEFT, 1);
    assert_eq!(linux.more_options(false), []);
    run(&mut state, Command::Properties);
    assert_eq!(state.more_options(false), []);
}

#[test]
fn what_windows_own_menu_chose_the_app_does_with_its_own_commands() {
    let mut state = with_windows_menu();
    // The menu of the rows `rows`, chosen: the token of its ask.
    let ask = |state: &mut AppState, rows: &[usize]| {
        choose(state, rows);
        token_of(&run(state, Command::SystemMenu))
    };
    let chosen = |state: &mut AppState, token, verb| {
        state.system_menu_chosen(token, SystemMenuChoice::App(verb))
    };
    // 삭제: the trash, through the app's job (the folder's trash is looked for first).
    let token = ask(&mut state, &[1, 2]);
    assert_eq!(
        chosen(&mut state, token, AppVerb::Delete { permanent: false }),
        [Effect::CheckTrash {
            folder: folder("a"),
            paths: vec![in_a("a.txt"), in_a("b.txt")],
        }]
    );
    // Once: a second answer, or one no menu is waiting for, does nothing.
    assert_eq!(
        chosen(&mut state, token, AppVerb::Delete { permanent: false }),
        []
    );
    // With Shift, for good: the confirmation first.
    let token = ask(&mut state, &[1]);
    assert_eq!(
        chosen(&mut state, token, AppVerb::Delete { permanent: true }),
        []
    );
    assert!(
        matches!(state.modal(), Some(Modal::Confirm(_))),
        "{:?}",
        state.modal()
    );
    state.dialog_cancelled();
    // 이름 바꾸기: the app's name window.
    let token = ask(&mut state, &[2]);
    assert_eq!(chosen(&mut state, token, AppVerb::Rename), []);
    assert!(
        matches!(state.modal(), Some(Modal::Name(_))),
        "{:?}",
        state.modal()
    );
    state.dialog_cancelled();
    // 잘라내기 and 복사: the system clipboard, through the app's own.
    for (verb, cut) in [(AppVerb::Cut, true), (AppVerb::Copy, false)] {
        let token = ask(&mut state, &[1, 2]);
        match the_work(&chosen(&mut state, token, verb)) {
            ClipboardWork::PutFiles {
                paths, cut: put, ..
            } => {
                assert_eq!(paths, [in_a("a.txt"), in_a("b.txt")], "{verb:?}");
                assert_eq!(put, cut, "{verb:?}");
            }
            other => panic!("{verb:?}: a put, not {other:?}"),
        }
    }
    // 붙여넣기 in the menu of a folder: into it, as the app's menu of the folder pastes; in the
    // folder's own menu, into the pane's folder.
    let pasted_to = |state: &mut AppState, effects: &[Effect]| -> VPath {
        let ClipboardWork::ReadFiles { token } = the_work(effects) else {
            panic!("a read, not {effects:?}");
        };
        let elsewhere =
            VPath::local(std::env::temp_dir().join("c").join("z.txt")).expect("a local path");
        let effects = state.clipboard_read(
            token,
            Ok(Some(files(&[elsewhere], Some(DropEffect::Copy), 1))),
        );
        match the_submit(&effects).0 {
            JobSpec::Transfer(spec) => spec.items[0].target.parent().expect("a folder"),
            other => panic!("a transfer, not {other:?}"),
        }
    };
    state.row_menu(LEFT, 0);
    let asked = state.more_options(false);
    assert_eq!(
        untokened(asked.clone()),
        [system_menu(&["docs"], MenuPlace::Shown, false)]
    );
    let effects = chosen(&mut state, token_of(&asked), AppVerb::Paste);
    assert_eq!(pasted_to(&mut state, &effects), in_a("docs"));
    let asked = state.more_options(true);
    assert_eq!(
        untokened(asked.clone()),
        [system_menu(&[], MenuPlace::Shown, false)]
    );
    let effects = chosen(&mut state, token_of(&asked), AppVerb::Paste);
    assert_eq!(pasted_to(&mut state, &effects), folder("a"));
    // The folder's own menu has none of the items' commands: nothing is renamed or deleted.
    for verb in [
        AppVerb::Rename,
        AppVerb::Delete { permanent: false },
        AppVerb::Delete { permanent: true },
        AppVerb::Cut,
        AppVerb::Copy,
    ] {
        let token = token_of(&state.more_options(true));
        assert_eq!(chosen(&mut state, token, verb), [], "{verb:?}");
        assert_eq!(state.modal(), None, "{verb:?}");
    }
    // What the system did itself, and nothing chosen: nothing for the app.
    for choice in [SystemMenuChoice::Done, SystemMenuChoice::Nothing] {
        let token = ask(&mut state, &[1]);
        assert_eq!(state.system_menu_chosen(token, choice), [], "{choice:?}");
        // The menu is answered: a late command of it does nothing.
        assert_eq!(chosen(&mut state, token, AppVerb::Rename), []);
        assert_eq!(state.modal(), None);
    }
}

#[test]
fn what_windows_own_menu_chose_acts_only_on_what_the_menu_was_for() {
    let mut state = with_windows_menu();
    let ask = |state: &mut AppState, rows: &[usize]| {
        choose(state, rows);
        token_of(&run(state, Command::SystemMenu))
    };
    let trash = AppVerb::Delete { permanent: false };
    let chosen = |state: &mut AppState, token, verb| {
        state.system_menu_chosen(token, SystemMenuChoice::App(verb))
    };
    let moved = || vec![Effect::Notice(words::system_menu_moved().to_owned())];
    // The menu was of a.txt; by its answer the selection is b.txt: nothing is trashed, and a
    // notice says why.
    let token = ask(&mut state, &[1]);
    choose(&mut state, &[2]);
    assert_eq!(chosen(&mut state, token, trash), moved());
    assert_eq!(state.modal(), None);
    // The same for 이름 바꾸기, which would rename b.txt, and for more items than the menu's.
    let token = ask(&mut state, &[1]);
    choose(&mut state, &[1, 2]);
    assert_eq!(chosen(&mut state, token, AppVerb::Rename), moved());
    assert_eq!(state.modal(), None);
    // The answer of a menu asked before the last one is stale: nothing, and the last one's
    // answer still comes.
    let first = ask(&mut state, &[1]);
    let last = ask(&mut state, &[2]);
    assert_eq!(chosen(&mut state, first, trash), []);
    assert_eq!(
        chosen(&mut state, last, trash),
        [Effect::CheckTrash {
            folder: folder("a"),
            paths: vec![in_a("b.txt")],
        }]
    );
    // The folder's own menu: by its answer the pane shows another folder, where nothing is
    // pasted.
    choose(&mut state, &[]);
    let token = token_of(&run(&mut state, Command::SystemMenu));
    let effects = state.go_to(folder("b"));
    assert!(!effects.is_empty(), "{effects:?}");
    assert_eq!(chosen(&mut state, token, AppVerb::Paste), moved());
    // Nothing chosen, or a command the system did itself, needs no notice.
    let token = ask(&mut state, &[]);
    let _ = state.go_to(folder("a"));
    assert_eq!(state.system_menu_chosen(token, SystemMenuChoice::Done), []);
}

#[test]
fn windows_own_menu_shows_on_its_turn_only_while_nothing_has_changed() {
    let mut state = with_windows_menu();
    let ask = |state: &mut AppState, rows: &[usize]| {
        choose(state, rows);
        token_of(&run(state, Command::SystemMenu))
    };
    // Asked a turn of the event loop before, as the controller shows it: nothing has changed,
    // and it shows.
    let token = ask(&mut state, &[1]);
    assert!(state.system_menu_due(token));
    // A menu asked before the last one does not show.
    let first = ask(&mut state, &[1]);
    let last = ask(&mut state, &[2]);
    assert!(!state.system_menu_due(first));
    assert!(state.system_menu_due(last));
    // A dialog came in the meantime (a job's question): the menu does not show over it, and is
    // forgotten; nothing of it is done later.
    let token = ask(&mut state, &[1]);
    run(&mut state, Command::Properties);
    assert!(state.modal().is_some());
    assert!(!state.system_menu_due(token));
    state.dialog_cancelled();
    assert!(!state.system_menu_due(token), "forgotten");
    assert_eq!(
        state.system_menu_chosen(token, SystemMenuChoice::App(AppVerb::Rename)),
        []
    );
    assert_eq!(state.modal(), None);
    // The items a command acts on are not the menu's any more, or the pane shows another
    // folder (a key, a listing, a folder's watch): it does not show.
    let token = ask(&mut state, &[1]);
    choose(&mut state, &[2]);
    assert!(!state.system_menu_due(token));
    let token = ask(&mut state, &[]);
    let effects = state.go_to(folder("b"));
    assert!(!effects.is_empty(), "{effects:?}");
    assert!(!state.system_menu_due(token));
    // Nor while the app ends.
    let _ = state.go_to(folder("a"));
    let token = ask(&mut state, &[]);
    assert_eq!(state.request_close(), [Effect::Quit]);
    assert!(!state.system_menu_due(token));
}

#[test]
fn share_hands_the_items_a_command_acts_on_to_the_system_s_picker() {
    let mut state = listed();
    state.row_menu(LEFT, 1);
    assert_eq!(state.share(), [Effect::Share(vec![in_a("a.txt")])]);
    choose(&mut state, &[0, 2]);
    assert_eq!(
        state.share(),
        [Effect::Share(vec![in_a("docs"), in_a("b.txt")])]
    );
    // Nothing chosen, or a window over the main one: nothing.
    choose(&mut state, &[]);
    assert_eq!(state.share(), []);
    choose(&mut state, &[1]);
    run(&mut state, Command::Properties);
    assert_eq!(state.share(), []);
}
