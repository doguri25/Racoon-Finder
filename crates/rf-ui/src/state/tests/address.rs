//! The address bar (NAV-03): what typed text names, the list of a piece's subfolders under its
//! `›`, and the completion list of the path field.

use std::fmt::Write as _;
use std::path::{MAIN_SEPARATOR, Path};

use super::*;
use crate::keys::ListKey;
use crate::loader::{FolderName, NamesRequest};
use crate::state::address::encode_url_name;

/// A lookup of the variables `vars`.
fn vars<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |name| {
        vars.iter()
            .find(|(each, _)| *each == name)
            .map(|(_, value)| (*value).to_owned())
    }
}

#[test]
fn home_and_variables_expand_by_the_rules_of_each_system() {
    let unix = [
        ("HOME", "/home/me"),
        ("PROJECT", "work/rf"),
        ("LOOP", "$HOME"),
    ];
    let unix = vars(&unix);
    for os in [Os::MacOs, Os::Other] {
        let x = |text: &str| expand_user_path(text, os, &unix);
        assert_eq!(x("~"), "/home/me");
        assert_eq!(x("~/docs"), "/home/me/docs");
        assert_eq!(x("$HOME/docs"), "/home/me/docs");
        assert_eq!(x("${HOME}/docs"), "/home/me/docs");
        assert_eq!(x("/srv/$PROJECT/x"), "/srv/work/rf/x");
        assert_eq!(x("/srv/${PROJECT}x"), "/srv/work/rfx");
        // A value is not expanded again.
        assert_eq!(x("$LOOP"), "$HOME");
        // An unknown variable stays as typed, and so does what is no variable: a name runs as
        // far as letters, digits and `_` go, and starts with no digit.
        assert_eq!(x("$NOPE/x"), "$NOPE/x");
        assert_eq!(x("${NOPE}/x"), "${NOPE}/x");
        assert_eq!(x("$HOMEX"), "$HOMEX");
        assert_eq!(x("a$"), "a$");
        assert_eq!(x("$1/x"), "$1/x");
        assert_eq!(x("${HOME"), "${HOME");
        assert_eq!(x("${}"), "${}");
        assert_eq!(x("${A B}"), "${A B}");
        // `~` is home only on its own at the start; another user's home is not looked up.
        assert_eq!(x("~me/x"), "~me/x");
        assert_eq!(x("/a/~/b"), "/a/~/b");
        assert_eq!(x(r"~\x"), r"~\x");
        // Windows' form means nothing here.
        assert_eq!(x("%HOME%/x"), "%HOME%/x");
    }
    let windows = [
        ("USERPROFILE", r"C:\Users\me"),
        ("APPDATA", r"C:\Users\me\AppData\Roaming"),
        ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        ("LOOP", "%USERPROFILE%"),
    ];
    let windows = vars(&windows);
    let x = |text: &str| expand_user_path(text, Os::Windows, &windows);
    assert_eq!(x(r"%USERPROFILE%\Documents"), r"C:\Users\me\Documents");
    assert_eq!(x("%APPDATA%"), r"C:\Users\me\AppData\Roaming");
    assert_eq!(
        x(r"%ProgramFiles(x86)%\Tool"),
        r"C:\Program Files (x86)\Tool"
    );
    assert_eq!(x("%LOOP%"), "%USERPROFILE%");
    assert_eq!(x("~"), r"C:\Users\me");
    assert_eq!(x(r"~\Desktop"), r"C:\Users\me\Desktop");
    assert_eq!(x("~/Desktop"), r"C:\Users\me/Desktop");
    // An unknown variable keeps both of its `%`, and what follows is read on from there.
    assert_eq!(x(r"%NOPE%\x"), r"%NOPE%\x");
    assert_eq!(
        x(r"%NOPE%\%APPDATA%"),
        r"%NOPE%\C:\Users\me\AppData\Roaming"
    );
    assert_eq!(x("100%"), "100%");
    assert_eq!(x("%%"), "%%");
    // Unix forms mean nothing on Windows.
    assert_eq!(x("$HOME"), "$HOME");
    assert_eq!(x("${HOME}"), "${HOME}");
    // Without a home folder, `~` stays as typed.
    assert_eq!(expand_user_path("~/x", Os::Other, &vars(&[])), "~/x");
    assert_eq!(expand_user_path(r"~\x", Os::Windows, &vars(&[])), r"~\x");
}

#[test]
fn addresses_of_remote_places_are_told_apart_from_paths() {
    for remote in [
        "sftp://host/dir",
        "SFTP://host",
        "smb://server/share",
        "ftp://example.org/x",
        "https://example.org",
        "webdav+https://host/x",
        "remote://profile/dir",
    ] {
        assert!(is_remote_url(remote), "{remote}");
    }
    for path in [
        "/home/me",
        r"C:\Users",
        "C://Users",
        r"\\server\share",
        "local:/home/me",
        "relative/dir",
        "://x",
        "a:b://c",
        "1ab://x",
        "/srv/a://b",
        // A file URL is read as the path it names (`read_file_url`).
        "file:///C:/x",
        "FILE://server/share",
    ] {
        assert!(!is_remote_url(path), "{path}");
    }
}

#[test]
fn file_urls_name_local_paths_on_each_system() {
    let path = |text: &str| FileUrl::Path(text.to_owned());
    let windows = |url: &str| read_file_url(url, Os::Windows);
    // A drive: decoded, with `\` between names.
    assert_eq!(
        windows("file:///C:/Users/me%20too/%ED%95%9C%EA%B8%80"),
        Some(path(r"C:\Users\me too\한글"))
    );
    assert_eq!(windows("file:///C:"), Some(path(r"C:\")));
    assert_eq!(windows("file:///C:/"), Some(path(r"C:\")));
    assert_eq!(windows("file:///c|/x"), Some(path(r"c:\x")));
    assert_eq!(windows("file://localhost/D:/x"), Some(path(r"D:\x")));
    assert_eq!(windows("FILE:///C:/x?query#part"), Some(path(r"C:\x")));
    // A computer: its share.
    assert_eq!(
        windows("file://server/share/x%20y"),
        Some(path(r"\\server\share\x y"))
    );
    assert_eq!(
        windows("file:////server/share/x"),
        Some(path(r"\\server\share\x"))
    );
    // No drive, or a drive with no root.
    assert_eq!(windows("file:///home/me"), Some(FileUrl::Invalid));
    assert_eq!(windows("file:///C:x"), Some(FileUrl::Invalid));
    // An encoded separator, in the path or the computer's name, is no path.
    for url in [
        "file:///C:/a%2Fb",
        "file:///C:/a%2fb",
        "file:///C:/a%5Cb",
        "file:///C:/a%5cb",
        "file://ser%2Fver/share",
        "file://ser%5Cver/share",
    ] {
        assert_eq!(windows(url), Some(FileUrl::Invalid), "{url}");
    }
    for os in [Os::MacOs, Os::Other] {
        let unix = |url: &str| read_file_url(url, os);
        assert_eq!(unix("file:///home/me%20too"), Some(path("/home/me too")));
        assert_eq!(unix("file://localhost/home"), Some(path("/home")));
        assert_eq!(unix("file://LOCALHOST/home"), Some(path("/home")));
        assert_eq!(unix("file:/home/x"), Some(path("/home/x")));
        assert_eq!(unix("file://"), Some(path("/")));
        assert_eq!(unix("file:///home/%ED%95%9C"), Some(path("/home/한")));
        // A `%` that starts no `%XX` stays; bytes that are no text are no path.
        assert_eq!(unix("file:///home/50%"), Some(path("/home/50%")));
        assert_eq!(unix("file:///home/%zz"), Some(path("/home/%zz")));
        assert_eq!(unix("file:///home/%FF"), Some(FileUrl::Invalid));
        for url in [
            "file:///home/a%2Fb",
            "file:///home/a%5cb",
            "file://local%2Fhost/x",
        ] {
            assert_eq!(unix(url), Some(FileUrl::Invalid), "{url}");
        }
        // Another computer is remote.
        assert_eq!(unix("file://server/share"), Some(FileUrl::Remote));
        assert_eq!(unix("file:x"), Some(FileUrl::Invalid));
    }
    // Other text is no file URL.
    for text in [
        "/home/me",
        "files:///x",
        "file",
        "fil",
        r"C:\x",
        "sftp://host/x",
    ] {
        assert_eq!(read_file_url(text, Os::Other), None, "{text}");
        assert_eq!(read_file_url(text, Os::Windows), None, "{text}");
    }
}

/// The `file:` URL of the local path `path`, every byte but letters, digits and `/-_.~:`
/// percent-encoded.
fn file_url(path: &Path) -> String {
    let text = path
        .to_str()
        .expect("a temporary path is text")
        .replace('\\', "/");
    let mut url = String::from(if text.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~:".contains(&byte) {
            url.push(char::from(byte));
        } else {
            let _ = write!(url, "%{byte:02X}");
        }
    }
    url
}

#[test]
fn a_file_url_is_read_before_anything_is_expanded() {
    // Its `%20` and `%25` are no `%VAR%`, and a `$HOME` it names is a folder's name.
    let folder = std::env::temp_dir().join("50% off $HOME");
    let env = [
        ("HOME", "/nowhere"),
        ("USERPROFILE", r"C:\nowhere"),
        ("25", "BAD"),
        ("20off", "BAD"),
    ];
    let env = vars(&env);
    let url = file_url(&folder);
    assert!(url.contains("%20off%20%24HOME"), "{url}");
    assert_eq!(
        read_typed(&url, Os::CURRENT, &env),
        Typed::Folder(VPath::local(&folder).expect("a local path"))
    );
}

#[test]
fn typed_text_names_a_folder_a_remote_place_or_nothing() {
    let home = folder("home");
    let home_text = home.to_string();
    let env = [
        ("HOME", home_text.as_str()),
        ("USERPROFILE", home_text.as_str()),
    ];
    let env = vars(&env);
    let read = |text: &str| read_typed(text, Os::CURRENT, &env);
    assert_eq!(
        read(&format!("  {home_text}  ")),
        Typed::Folder(home.clone())
    );
    assert_eq!(read("~"), Typed::Folder(home.clone()));
    assert_eq!(
        read(&format!("~{MAIN_SEPARATOR}docs")),
        Typed::Folder(home.join("docs").expect("a name"))
    );
    assert_eq!(read("sftp://host/dir"), Typed::Remote);
    assert_eq!(read("relative/dir"), Typed::Invalid);
    assert_eq!(read("$NOPE/x"), Typed::Invalid);
    assert_eq!(read("%NOPE%/x"), Typed::Invalid);
}

#[test]
fn completion_splits_after_the_last_separator() {
    assert_eq!(
        split_last("/home/me/Do", Os::Other),
        Some(("/home/me/", "Do"))
    );
    assert_eq!(split_last("/home/me/", Os::MacOs), Some(("/home/me/", "")));
    assert_eq!(split_last("/", Os::Other), Some(("/", "")));
    assert_eq!(split_last(r"/a\b", Os::Other), Some(("/", r"a\b")));
    assert_eq!(split_last("한글/폴더", Os::Other), Some(("한글/", "폴더")));
    assert_eq!(
        split_last(r"C:\Users\me\Do", Os::Windows),
        Some((r"C:\Users\me\", "Do"))
    );
    assert_eq!(
        split_last(r"C:\Users/me", Os::Windows),
        Some((r"C:\Users/", "me"))
    );
    assert_eq!(split_last("C:", Os::Windows), None);
    assert_eq!(split_last("docs", Os::Other), None);
}

fn names(list: &[&str]) -> Vec<FolderName> {
    list.iter().map(|name| FolderName::folder(*name)).collect()
}

/// The one listing of names among `effects`.
fn the_names_request(effects: &[Effect]) -> NamesRequest {
    let found: Vec<&NamesRequest> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::ListNames(request) => Some(request),
            _ => None,
        })
        .collect();
    match found.as_slice() {
        [request] => (*request).clone(),
        other => panic!("expected one listing of names, got {other:?} in {effects:?}"),
    }
}

/// The names `list`, and `more` left out, come for `request`.
fn names_come(
    state: &mut AppState,
    request: &NamesRequest,
    list: &[&str],
    more: usize,
) -> Vec<Effect> {
    state.loaded(LoadEvent::Names {
        generation: request.generation,
        names: names(list),
        more,
        links_left: false,
    })
}

/// The rows a page of the list of subfolders has.
const PAGE: usize = 10;

/// What the controller does after every change.
fn settle(state: &mut AppState) {
    state.drop_stale_address();
}

/// The last piece of the path row of pane `side`: its folder itself.
fn last_piece(state: &AppState, side: usize) -> usize {
    ancestors(state.panes()[side].active_tab().location()).len() - 1
}

fn listed(names: &Names) -> Vec<&str> {
    names
        .rows()
        .iter()
        .map(|each| each.name.to_str().expect("test names are text"))
        .collect()
}

fn subfolder_rows(state: &AppState) -> Vec<&str> {
    state
        .subfolders()
        .map(|list| listed(list.names()))
        .unwrap_or_default()
}

fn completion_rows(state: &AppState, side: usize) -> Vec<&str> {
    state
        .completion(side)
        .map(|completion| listed(completion.names()))
        .unwrap_or_default()
}

fn in_folder(name: &str, child: &str) -> VPath {
    folder(name).join(child).expect("a name")
}

/// Opens the list of the folders in the folder pane `side` shows, which lists `list`.
fn open_listed(state: &mut AppState, side: usize, list: &[&str]) -> NamesRequest {
    let piece = last_piece(state, side);
    let request = the_names_request(&state.open_subfolders(side, piece));
    names_come(state, &request, list, 0);
    settle(state);
    assert!(state.subfolders().is_some());
    request
}

#[test]
fn a_pieces_chevron_lists_its_folders_and_opens_the_one_chosen() {
    let mut state = ready(vec![], vec![]);
    let piece = last_piece(&state, RIGHT);
    let request = the_names_request(&state.open_subfolders(RIGHT, piece));
    assert_eq!(state.active(), RIGHT);
    assert_eq!(request.folder, folder("b"));
    assert_eq!(request.prefix, "");
    assert_eq!(request.limit, MAX_NAMES);
    assert!(!request.filter.show_hidden);
    assert!(state.generations().contains(&request.generation));
    let list = state.subfolders().expect("the list shows while it loads");
    assert_eq!((list.side(), list.index()), (RIGHT, piece));
    assert_eq!(list.names(), &Names::Loading);
    // The names reach the list, and nothing else.
    assert_eq!(names_come(&mut state, &request, &["docs", "src"], 0), []);
    assert_eq!(subfolder_rows(&state), ["docs", "src"]);
    assert_eq!(rows(&state, RIGHT), Vec::<&str>::new());
    let cursor = |state: &AppState| state.subfolders().and_then(Subfolders::cursor);
    // ↓ from no row goes to the first one; the cursor stops at either end.
    assert_eq!(cursor(&state), None);
    state.subfolders_key(ListKey::Down, PAGE);
    assert_eq!(cursor(&state), Some(0));
    state.subfolders_key(ListKey::Up, PAGE);
    assert_eq!(cursor(&state), Some(0));
    state.subfolders_key(ListKey::Down, PAGE);
    state.subfolders_key(ListKey::Down, PAGE);
    assert_eq!(cursor(&state), Some(1));
    let effects = state.subfolders_key(ListKey::Choose, PAGE);
    assert_eq!(the_load(&effects).path, in_folder("b", "src"));
    assert!(state.subfolders().is_none());
    assert!(!state.generations().contains(&request.generation));

    // The root's piece lists the root; ↑ from no row goes to the last row, and a click on a
    // row goes to its folder.
    let request = the_names_request(&state.open_subfolders(LEFT, 0));
    assert_eq!(state.active(), LEFT);
    let root = ancestors(&folder("a")).remove(0);
    assert_eq!(request.folder, root);
    names_come(&mut state, &request, &["tmp", "usr"], 0);
    state.subfolders_key(ListKey::Up, PAGE);
    assert_eq!(cursor(&state), Some(1));
    let effects = state.choose_subfolder(0);
    assert_eq!(the_load(&effects).path, root.join("tmp").expect("a name"));
    assert!(state.subfolders().is_none());

    // With hidden items shown, hidden folders are listed too; Esc closes the list.
    press(&mut state, "h", CTRL);
    let request = the_names_request(&state.open_subfolders(LEFT, 0));
    assert!(request.filter.show_hidden);
    state.subfolders_key(ListKey::Close, PAGE);
    assert!(state.subfolders().is_none());
    assert!(!state.generations().contains(&request.generation));
}

#[test]
fn a_late_listing_of_names_is_dropped_and_a_failed_one_shows() {
    let mut state = ready(vec![dir("docs")], vec![]);
    let first = the_names_request(&state.open_subfolders(LEFT, 0));
    let piece = last_piece(&state, LEFT);
    let second = the_names_request(&state.open_subfolders(LEFT, piece));
    assert!(!state.generations().contains(&first.generation));
    assert!(state.generations().contains(&second.generation));
    // The first piece's names come late: they are dropped, and reach neither the pane nor the
    // tree.
    assert_eq!(names_come(&mut state, &first, &["late"], 0), []);
    assert_eq!(
        state.subfolders().map(Subfolders::names),
        Some(&Names::Loading)
    );
    assert_eq!(rows(&state, LEFT), ["docs"]);
    // A listing that failed says so in the list, which stays until it is closed.
    state.loaded(LoadEvent::Failed {
        generation: second.generation,
        kind: ErrorKind::PermissionDenied,
    });
    settle(&mut state);
    assert_eq!(
        state.subfolders().map(Subfolders::names),
        Some(&Names::Failed(ErrorKind::PermissionDenied))
    );
    // Enter with no row does nothing.
    assert_eq!(state.subfolders_key(ListKey::Choose, PAGE), []);
    assert!(state.subfolders().is_some());
    state.close_subfolders();
    assert!(state.subfolders().is_none());
}

#[test]
fn the_list_of_subfolders_closes_when_what_it_belongs_to_changes() {
    let mut state = ready(vec![dir("docs")], vec![dir("docs")]);
    // Any command of the window closes it: a new tab here.
    open_listed(&mut state, LEFT, &["docs"]);
    press(&mut state, "t", CTRL);
    assert!(state.subfolders().is_none());
    // Another tab shows in its pane.
    open_listed(&mut state, LEFT, &["docs"]);
    state.select_tab(LEFT, 0);
    settle(&mut state);
    assert!(state.subfolders().is_none());
    // The other pane takes the keys.
    open_listed(&mut state, LEFT, &["docs"]);
    state.activate(RIGHT);
    settle(&mut state);
    assert!(state.subfolders().is_none());
    // Its pane goes to another folder (a row opened by a double click).
    open_listed(&mut state, RIGHT, &["docs"]);
    state.open_row(RIGHT, 0);
    settle(&mut state);
    assert!(state.subfolders().is_none());
    // The layout changes: the divider is dragged, the panes swap, one pane shows.
    open_listed(&mut state, RIGHT, &[]);
    state.set_split_share(0.3);
    settle(&mut state);
    assert!(state.subfolders().is_none());
    open_listed(&mut state, RIGHT, &[]);
    press(&mut state, "u", CTRL);
    settle(&mut state);
    assert!(state.subfolders().is_none());
    open_listed(&mut state, RIGHT, &[]);
    press(&mut state, &special(Key::F12), NONE);
    settle(&mut state);
    assert!(state.subfolders().is_none());
    // A dialog over the window: no list opens under it.
    press(&mut state, "n", CTRL_SHIFT);
    assert!(state.modal().is_some());
    assert_eq!(state.open_subfolders(RIGHT, 0), []);
    assert!(state.subfolders().is_none());
    state.dialog_cancelled();
    // Nothing changed: it stays.
    open_listed(&mut state, RIGHT, &[]);
    settle(&mut state);
    assert!(state.subfolders().is_some());
}

#[test]
fn a_folder_chosen_in_the_address_bar_opens_beside_a_pinned_tab_and_moves_one_pane() {
    let mut state = ready(vec![dir("docs")], vec![dir("docs")]);
    state.toggle_pin(LEFT, 0);
    press(&mut state, &special(Key::F9), SHIFT);
    assert!(state.syncing());
    open_listed(&mut state, LEFT, &["docs"]);
    let effects = state.choose_subfolder(0);
    // The pinned tab stays; a tab beside it goes in. The other pane stays, and sync stays on.
    assert_eq!(the_load(&effects).path, in_folder("a", "docs"));
    let pane = &state.panes()[LEFT];
    assert_eq!(pane.tabs().len(), 2);
    assert!(pane.tabs()[0].pinned());
    assert_eq!(pane.tabs()[0].location(), &folder("a"));
    assert_eq!(pane.active(), 1);
    assert_eq!(pane.active_tab().location(), &in_folder("a", "docs"));
    assert!(state.syncing());
    assert_eq!(state.panes()[RIGHT].active_tab().location(), &folder("b"));

    // A folder of the completion list goes the same way.
    state.select_tab(LEFT, 0);
    let typed = format!("{}{MAIN_SEPARATOR}d", folder("a"));
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    names_come(&mut state, &request, &["docs"], 0);
    state.completion_step(LEFT, 1);
    let effects = state.enter_path(LEFT, &typed);
    assert_eq!(the_load(&effects).path, in_folder("a", "docs"));
    let pane = &state.panes()[LEFT];
    assert_eq!(pane.tabs().len(), 3);
    assert_eq!(pane.tabs()[0].location(), &folder("a"));
    assert_eq!(pane.active(), 1);
    assert!(state.syncing());
    assert_eq!(state.panes()[RIGHT].active_tab().location(), &folder("b"));
}

#[test]
fn the_path_field_completes_the_folders_after_its_last_separator() {
    let mut state = ready(vec![], vec![]);
    let a = folder("a").to_string();
    let typed = format!("{a}{MAIN_SEPARATOR}d");
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    assert_eq!(request.folder, folder("a"));
    assert_eq!(request.prefix, "d");
    assert_eq!(request.limit, MAX_NAMES);
    assert_eq!(
        state.completion(LEFT).map(Completion::names),
        Some(&Names::Loading)
    );
    assert_eq!(state.completion(RIGHT), None);

    // A longer name before the answer asks again and drops the request before.
    let longer = format!("{a}{MAIN_SEPARATOR}do");
    let first = request;
    let request = the_names_request(&state.path_edited(LEFT, &longer));
    assert_eq!(request.prefix, "do");
    assert!(!state.generations().contains(&first.generation));
    assert!(state.generations().contains(&request.generation));
    // The answer for the text before is dropped; the one for this text shows.
    names_come(&mut state, &first, &["docs", "downloads"], 0);
    assert_eq!(completion_rows(&state, LEFT), Vec::<&str>::new());
    names_come(&mut state, &request, &["docs", "downloads"], 0);
    assert_eq!(completion_rows(&state, LEFT), ["docs", "downloads"]);
    // That answer is all the folder has for "do": a longer start narrows it here, with no new
    // listing.
    let narrower = format!("{a}{MAIN_SEPARATOR}doc");
    assert_eq!(state.path_edited(LEFT, &narrower), []);
    assert_eq!(completion_rows(&state, LEFT), ["docs"]);
    assert!(state.generations().contains(&request.generation));
    // A shorter start asks again; meanwhile the names already found that still fit show.
    let second = the_names_request(&state.path_edited(LEFT, &longer));
    assert_eq!(second.prefix, "do");
    assert_eq!(completion_rows(&state, LEFT), ["docs"]);
    names_come(&mut state, &second, &["docs", "doctor"], 3);
    assert_eq!(
        state.completion(LEFT).map(Completion::names),
        Some(&Names::Listed {
            names: names(&["docs", "doctor"]),
            more: 3
        })
    );
    // A listing that left folders out is narrowed only while a new one is out.
    let third = the_names_request(&state.path_edited(LEFT, &narrower));
    assert_eq!(third.prefix, "doc");
    assert_eq!(completion_rows(&state, LEFT), ["docs", "doctor"]);
    names_come(&mut state, &third, &["docs", "doctor"], 3);
    // So is one of another folder.
    let elsewhere = format!("{}{MAIN_SEPARATOR}doc", folder("b"));
    let fourth = the_names_request(&state.path_edited(LEFT, &elsewhere));
    assert_eq!(fourth.folder, folder("b"));
    let fifth = the_names_request(&state.path_edited(LEFT, &narrower));
    names_come(&mut state, &fifth, &["docs", "doctor"], 3);
    assert_eq!(completion_rows(&state, LEFT), ["docs", "doctor"]);

    // ↓ twice: the second row. Tab completes the text with its name and a separator, which
    // lists that folder next.
    state.completion_step(LEFT, 1);
    state.completion_step(LEFT, 1);
    state.completion_step(LEFT, 1);
    assert_eq!(state.completion(LEFT).and_then(Completion::cursor), Some(1));
    assert_eq!(state.complete_path(LEFT, "another text"), None);
    let (completed, effects) = state
        .complete_path(LEFT, &narrower)
        .expect("a name to complete with");
    assert_eq!(
        completed,
        format!("{a}{MAIN_SEPARATOR}doctor{MAIN_SEPARATOR}")
    );
    let next = the_names_request(&effects);
    assert_eq!(next.folder, in_folder("a", "doctor"));
    assert_eq!(next.prefix, "");
    assert_eq!(
        state.completion(LEFT).map(Completion::text),
        Some(completed.as_str())
    );
    // With no row chosen, Tab takes the first one.
    names_come(&mut state, &next, &["2025", "2026"], 0);
    let (completed, _) = state
        .complete_path(LEFT, &completed)
        .expect("a name to complete with");
    assert_eq!(
        completed,
        format!("{a}{MAIN_SEPARATOR}doctor{MAIN_SEPARATOR}2025{MAIN_SEPARATOR}")
    );
    // A folder with no folder that fits has nothing to complete with.
    let request = the_names_request(&state.path_edited(LEFT, &format!("{a}{MAIN_SEPARATOR}x")));
    names_come(&mut state, &request, &[], 0);
    assert_eq!(completion_rows(&state, LEFT), Vec::<&str>::new());
    assert_eq!(
        state.complete_path(LEFT, &format!("{a}{MAIN_SEPARATOR}x")),
        None
    );
}

#[test]
fn enter_goes_to_the_completions_row_or_else_to_the_text() {
    let mut state = ready(vec![], vec![]);
    let typed = format!("{}{MAIN_SEPARATOR}d", folder("a"));
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    names_come(&mut state, &request, &["docs", "downloads"], 0);
    // With no row chosen, Enter goes to the text itself.
    let effects = state.enter_path(LEFT, &typed);
    assert_eq!(the_load(&effects).path, in_folder("a", "d"));
    assert_eq!(state.completion(LEFT), None);
    assert!(!state.generations().contains(&request.generation));

    // ↑ from no row goes to the last one, and Enter goes to its folder.
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    names_come(&mut state, &request, &["docs", "downloads"], 0);
    state.completion_step(LEFT, -1);
    assert_eq!(state.completion(LEFT).and_then(Completion::cursor), Some(1));
    let effects = state.enter_path(LEFT, &typed);
    assert_eq!(the_load(&effects).path, in_folder("a", "downloads"));

    // A click on a row goes to its folder.
    let request = the_names_request(&state.path_edited(RIGHT, &typed));
    assert_eq!(state.active(), RIGHT);
    names_come(&mut state, &request, &["docs", "downloads"], 0);
    let effects = state.choose_completion(RIGHT, 0);
    assert_eq!(the_load(&effects).path, in_folder("a", "docs"));
    assert_eq!(state.completion(RIGHT), None);

    // Esc closes the list, and its listing stops.
    let request = the_names_request(&state.path_edited(RIGHT, &typed));
    state.dismiss_completion(RIGHT);
    assert_eq!(state.completion(RIGHT), None);
    assert!(!state.generations().contains(&request.generation));

    // Text with no separator, a remote address and a head that names no folder list nothing.
    for text in ["docs", "sftp://host/dir", "relative/dir"] {
        assert_eq!(state.path_edited(RIGHT, text), [], "{text}");
        assert_eq!(state.completion(RIGHT), None, "{text}");
    }
}

#[test]
fn the_completion_list_closes_when_its_pane_changes() {
    let mut state = ready(vec![], vec![]);
    let typed = format!("{}{MAIN_SEPARATOR}", folder("a"));
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    names_come(&mut state, &request, &["docs"], 0);
    settle(&mut state);
    assert_eq!(completion_rows(&state, LEFT), ["docs"]);
    // The other pane takes the keys (a click there ends the edit).
    state.activate(RIGHT);
    settle(&mut state);
    assert_eq!(state.completion(LEFT), None);
    // Another tab shows in the pane.
    let _ = state.add_tab(LEFT);
    let _ = state.path_edited(LEFT, &typed);
    state.select_tab(LEFT, 0);
    settle(&mut state);
    assert_eq!(state.completion(LEFT), None);
}

#[test]
fn typed_paths_expand_home_and_variables_and_remote_ones_are_refused() {
    let mut state = ready(vec![], vec![]);
    let home = folder("home").to_string();
    state.set_environment(Environment::Fixed(vec![
        ("HOME".to_owned(), home.clone()),
        ("USERPROFILE".to_owned(), home),
        ("PROJECTS".to_owned(), folder("projects").to_string()),
    ]));
    assert_eq!(the_load(&state.enter_path(LEFT, "~")).path, folder("home"));
    let variable = if cfg!(windows) {
        "%PROJECTS%"
    } else {
        "$PROJECTS"
    };
    let effects = state.enter_path(LEFT, &format!("{variable}{MAIN_SEPARATOR}x"));
    assert_eq!(the_load(&effects).path, in_folder("projects", "x"));
    // An unknown variable stays as typed: here that makes a relative path.
    for unknown in ["$NOPE/x", "%NOPE%/x"] {
        assert_eq!(
            state.enter_path(LEFT, unknown),
            [Effect::Notice(
                "열 수 없습니다: 쓸 수 없는 이름이나 경로입니다".to_owned()
            )],
            "{unknown}"
        );
    }
    // A remote address is not opened.
    for remote in [
        "sftp://host/dir",
        "smb://server/share",
        "https://example.org",
    ] {
        assert_eq!(
            state.enter_path(LEFT, remote),
            [Effect::Notice("아직 지원하지 않는 위치입니다".to_owned())],
            "{remote}"
        );
    }
    // A file URL names its local folder.
    let effects = state.enter_path(LEFT, &file_url(&std::env::temp_dir().join("a b")));
    assert_eq!(the_load(&effects).path, folder("a b"));
    // The completion list reads the text the same way.
    let request = the_names_request(&state.path_edited(LEFT, &format!("~{MAIN_SEPARATOR}")));
    assert_eq!(request.folder, folder("home"));
    let url = file_url(&std::env::temp_dir().join("a b"));
    let request = the_names_request(&state.path_edited(LEFT, &format!("{url}/x%20y")));
    assert_eq!(request.folder, folder("a b"));
    assert_eq!(request.prefix, "x y");
}

#[test]
fn a_tab_before_the_first_names_waits_for_them() {
    let mut state = ready(vec![], vec![]);
    let a = folder("a").to_string();
    let typed = format!("{a}{MAIN_SEPARATOR}d");
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    // Tab before the names: nothing yet, and the field keeps the keys (`pane.slint`).
    assert_eq!(state.complete_path(LEFT, &typed), None);
    // The names come: the field gets the text completed with the first one, and that folder's
    // listing starts.
    let effects = names_come(&mut state, &request, &["docs", "downloads"], 0);
    let completed = format!("{a}{MAIN_SEPARATOR}docs{MAIN_SEPARATOR}");
    assert!(
        effects.contains(&Effect::FillPath {
            side: LEFT,
            text: completed.clone()
        }),
        "{effects:?}"
    );
    assert_eq!(the_names_request(&effects).folder, in_folder("a", "docs"));
    assert_eq!(
        state.completion(LEFT).map(Completion::text),
        Some(completed.as_str())
    );

    // A Tab for a text that has changed since does nothing when the names come.
    let x = format!("{a}{MAIN_SEPARATOR}x");
    let _ = the_names_request(&state.path_edited(LEFT, &x));
    assert_eq!(state.complete_path(LEFT, &x), None);
    let xy = format!("{a}{MAIN_SEPARATOR}xy");
    let request = the_names_request(&state.path_edited(LEFT, &xy));
    assert_eq!(names_come(&mut state, &request, &["xyz"], 0), []);
    // Nor when no folder fits, or the folder could not be listed.
    let q = format!("{a}{MAIN_SEPARATOR}q");
    let request = the_names_request(&state.path_edited(LEFT, &q));
    assert_eq!(state.complete_path(LEFT, &q), None);
    assert_eq!(names_come(&mut state, &request, &[], 0), []);
    let r = format!("{a}{MAIN_SEPARATOR}r");
    let request = the_names_request(&state.path_edited(LEFT, &r));
    assert_eq!(state.complete_path(LEFT, &r), None);
    let failed = state.loaded(LoadEvent::Failed {
        generation: request.generation,
        kind: ErrorKind::PermissionDenied,
    });
    assert_eq!(failed, []);
}

#[test]
fn the_list_of_subfolders_moves_by_pages_and_to_its_ends() {
    let mut state = ready(vec![], vec![]);
    let names: Vec<String> = (1..=25).map(|n| format!("여행 {n}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let cursor = |state: &AppState| state.subfolders().and_then(Subfolders::cursor);
    open_listed(&mut state, LEFT, &names);
    // From no row, PageDown goes a page from the first row; it stops at the last one.
    state.subfolders_key(ListKey::PageDown, PAGE);
    assert_eq!(cursor(&state), Some(9));
    state.subfolders_key(ListKey::PageDown, PAGE);
    assert_eq!(cursor(&state), Some(19));
    state.subfolders_key(ListKey::PageDown, PAGE);
    assert_eq!(cursor(&state), Some(24));
    state.subfolders_key(ListKey::PageUp, PAGE);
    assert_eq!(cursor(&state), Some(14));
    state.subfolders_key(ListKey::First, PAGE);
    assert_eq!(cursor(&state), Some(0));
    state.subfolders_key(ListKey::PageUp, PAGE);
    assert_eq!(cursor(&state), Some(0));
    state.subfolders_key(ListKey::Last, PAGE);
    assert_eq!(cursor(&state), Some(24));
    // From no row, PageUp goes a page from the last row; a page of the rows the list shows.
    open_listed(&mut state, LEFT, &names);
    state.subfolders_key(ListKey::PageUp, 4);
    assert_eq!(cursor(&state), Some(21));
    // A list with no row has no cursor.
    open_listed(&mut state, LEFT, &[]);
    for key in [ListKey::First, ListKey::Last, ListKey::PageDown] {
        state.subfolders_key(key, PAGE);
        assert_eq!(cursor(&state), None, "{key:?}");
    }
}

#[test]
fn a_new_layout_alone_closes_the_lists() {
    let mut state = ready(vec![], vec![]);
    open_listed(&mut state, LEFT, &["docs"]);
    // Nothing changed: the list stays.
    settle(&mut state);
    assert!(state.subfolders().is_some());
    let before = |state: &AppState| {
        let tab = state.panes()[LEFT].active_tab();
        (state.active(), tab.id(), tab.location().clone())
    };
    let was = before(&state);
    // The divider moves: the pane, its tab and its folder stay, and the list closes.
    state.set_split_share(0.3);
    settle(&mut state);
    assert_eq!(before(&state), was);
    assert!(state.subfolders().is_none());
    // So does the completion list.
    let typed = format!("{}{MAIN_SEPARATOR}", folder("a"));
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    names_come(&mut state, &request, &["docs"], 0);
    settle(&mut state);
    assert!(state.completion(LEFT).is_some());
    state.set_split_share(0.6);
    settle(&mut state);
    assert_eq!(before(&state), was);
    assert_eq!(state.completion(LEFT), None);
}

#[test]
fn the_completion_list_closes_when_the_panes_swap_or_one_shows() {
    let mut state = ready(vec![], vec![]);
    let typed = format!("{}{MAIN_SEPARATOR}", folder("a"));
    let edit = |state: &mut AppState| {
        let request = the_names_request(&state.path_edited(LEFT, &typed));
        names_come(state, &request, &["docs"], 0);
        settle(state);
        assert!(state.completion(LEFT).is_some());
    };
    edit(&mut state);
    press(&mut state, "u", CTRL);
    settle(&mut state);
    assert_eq!(state.completion(LEFT), None);
    edit(&mut state);
    press(&mut state, &special(Key::F12), NONE);
    settle(&mut state);
    assert!(!state.layout().dual);
    assert_eq!(state.completion(LEFT), None);
}

#[test]
fn a_link_to_a_folder_is_listed_and_opened_as_a_folder() {
    let mut state = ready(vec![], vec![]);
    let request = the_names_request(&state.open_subfolders(LEFT, 0));
    let link = FolderName {
        name: "bin".into(),
        link: true,
    };
    state.loaded(LoadEvent::Names {
        generation: request.generation,
        names: vec![link.clone(), FolderName::folder("etc")],
        more: 0,
        links_left: false,
    });
    assert_eq!(
        state.subfolders().map(|list| list.names().rows()),
        Some([link, FolderName::folder("etc")].as_slice())
    );
    let effects = state.choose_subfolder(0);
    assert_eq!(
        the_load(&effects).path,
        request.folder.join("bin").expect("a name")
    );
}

#[test]
fn a_name_completed_into_a_file_url_is_encoded() {
    // What a URL's name cannot hold as it is: `%`, `#`, `?`, spaces and controls.
    assert_eq!(
        encode_url_name("C# 50%?\u{1}\u{7f}프로젝트-1.2_x~"),
        "C%23%2050%25%3F%01%7F프로젝트-1.2_x~"
    );
    let mut state = ready(vec![], vec![]);
    let url = file_url(&std::env::temp_dir().join("a b"));
    let head = format!("{url}/");
    let request = the_names_request(&state.path_edited(LEFT, &head));
    assert_eq!(request.folder, folder("a b"));
    names_come(&mut state, &request, &["C# 50% 프로젝트"], 0);
    let (completed, effects) = state
        .complete_path(LEFT, &head)
        .expect("a name to complete with");
    assert_eq!(completed, format!("{head}C%23%2050%25%20프로젝트/"));
    // It reads back as that folder.
    let named = std::env::temp_dir().join("a b").join("C# 50% 프로젝트");
    assert_eq!(
        read_typed(&completed, Os::CURRENT, &vars(&[])),
        Typed::Folder(VPath::local(&named).expect("a local path"))
    );
    assert_eq!(
        the_names_request(&effects).folder,
        VPath::local(&named).expect("a local path")
    );
}

#[test]
fn a_listing_with_links_left_unexamined_is_not_complete() {
    let mut state = ready(vec![], vec![]);
    let a = folder("a").to_string();
    let typed = format!("{a}{MAIN_SEPARATOR}d");
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    // No folder was left out, but links past the limit were not looked at.
    state.loaded(LoadEvent::Names {
        generation: request.generation,
        names: names(&["docs"]),
        more: 0,
        links_left: true,
    });
    // So a longer name lists again.
    let longer = the_names_request(&state.path_edited(LEFT, &format!("{typed}o")));
    assert_eq!(longer.prefix, "do");
}

#[test]
fn a_tab_waits_while_a_request_for_the_text_is_out() {
    let mut state = ready(vec![], vec![]);
    let a = folder("a").to_string();
    let typed = format!("{a}{MAIN_SEPARATOR}d");
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    assert!(state.completion(LEFT).is_some_and(Completion::waiting));
    // A listing that left folders out.
    names_come(&mut state, &request, &["docs"], 5);
    assert!(!state.completion(LEFT).is_some_and(Completion::waiting));
    // A longer name: no row fits any more, and a request for it is out; Tab waits for it.
    let longer = format!("{a}{MAIN_SEPARATOR}dz");
    let request = the_names_request(&state.path_edited(LEFT, &longer));
    assert_eq!(completion_rows(&state, LEFT), Vec::<&str>::new());
    assert!(state.completion(LEFT).is_some_and(Completion::waiting));
    assert_eq!(state.complete_path(LEFT, &longer), None);
    let effects = names_come(&mut state, &request, &["dzz"], 0);
    assert!(
        effects.contains(&Effect::FillPath {
            side: LEFT,
            text: format!("{a}{MAIN_SEPARATOR}dzz{MAIN_SEPARATOR}")
        }),
        "{effects:?}"
    );
    // While rows still fit, Tab completes at once with them.
    let request = the_names_request(&state.path_edited(LEFT, &typed));
    names_come(&mut state, &request, &["docs", "dz"], 5);
    let _ = the_names_request(&state.path_edited(LEFT, &format!("{a}{MAIN_SEPARATOR}do")));
    assert!(state.completion(LEFT).is_some_and(Completion::waiting));
    let (completed, _) = state
        .complete_path(LEFT, &format!("{a}{MAIN_SEPARATOR}do"))
        .expect("a row to complete with");
    assert_eq!(
        completed,
        format!("{a}{MAIN_SEPARATOR}docs{MAIN_SEPARATOR}")
    );
}
