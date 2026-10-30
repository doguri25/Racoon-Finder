//! The item menu and the system's ways to open an item (OPS-04, OPS-06's system window,
//! OPS-11): what the menu offers for what is chosen, what each command asks of the system, the
//! menu of the apps that can open a file (macOS), and the item menu from the keyboard.

use std::path::PathBuf;

use rf_platform::shell::App;

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
        press(&mut state, &special(Key::F10), SHIFT).expect("the menu key"),
        [Effect::ShowMenu(MenuShown::Folder(LEFT))]
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
            one_folder: true,
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
