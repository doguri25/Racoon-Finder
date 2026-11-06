//! Making links (OPS-08): what the item menu's 링크 만들기 ▸ offers, the job it submits, the
//! picker that finds another folder for them, and the links a drop makes (DND-03). The rules of
//! every system are checked on any; what runs here is the system the tests run on.

use rf_core::Scheme;
use rf_jobs::{JobSpec, Outcome};
use rf_platform::dnd::{DragMods, DropEffect, DroppedItems, Effects};
use rf_platform::{LinkKind, drop_kind, kinds_for, link_name};

use super::drag::{RIGHT_LIST, ask_menu, in_a, link_keys, the_submit, toasts, two_panes};
use super::*;
use crate::dialogs::Modal;
use crate::jobs::{JobEffect, Started};
use crate::pick::{PickPurpose, Picker, TargetFacts};
use crate::state::drop_rule::DropChoice;
use crate::state::{LinkChoice, link_facts, link_job};
use crate::words::{self, JobWork};

fn choices(facts: LinkFacts) -> Vec<(LinkKind, bool)> {
    facts
        .choices()
        .map(|choice| (choice.kind, choice.enabled))
        .collect()
}

fn offered(os: Os, items: &[Entry], needs_privilege: bool) -> Vec<(LinkKind, bool)> {
    let items: Vec<(VPath, &Entry)> = items
        .iter()
        .map(|entry| (in_a(entry.name.to_str().unwrap()), entry))
        .collect();
    choices(link_facts(os, &items, needs_privilege))
}

#[test]
fn each_system_offers_its_own_kinds_for_what_is_chosen() {
    use LinkKind::{Junction, Shortcut, Symlink};
    // A file: a shortcut and a symbolic link; a folder on Windows also a junction.
    let (a_file, a_folder) = (file("a.txt"), dir("docs"));
    assert_eq!(
        offered(Os::Windows, std::slice::from_ref(&a_file), false),
        [(Shortcut, true), (Symlink, true)]
    );
    assert_eq!(
        offered(Os::Windows, std::slice::from_ref(&a_folder), false),
        [(Shortcut, true), (Symlink, true), (Junction, true)]
    );
    assert_eq!(
        offered(Os::MacOs, std::slice::from_ref(&a_folder), false),
        [(Shortcut, true), (Symlink, true)]
    );
    assert_eq!(
        offered(Os::Other, std::slice::from_ref(&a_folder), false),
        [(Symlink, true)]
    );
    // Several items: a junction needs every one to be a folder itself.
    assert_eq!(
        offered(Os::Windows, &[a_folder.clone(), dir("src")], false).len(),
        3
    );
    assert_eq!(
        offered(Os::Windows, &[a_folder.clone(), a_file], false).len(),
        2
    );
    // A link is not known to lead to a folder (the listing does not tell), so no junction.
    let link = Entry::new("l", EntryKind::Symlink);
    assert_eq!(offered(Os::Windows, &[link], false).len(), 2);
}

#[test]
fn a_symbolic_link_without_the_rights_for_one_is_offered_off() {
    use LinkKind::{Shortcut, Symlink};
    assert_eq!(
        offered(Os::Windows, &[file("a.txt")], true),
        [(Shortcut, true), (Symlink, false)]
    );
    // The title of the entry that is off says why; the others are plain.
    let off = words::link_entry(Symlink, false, Os::Windows);
    assert!(off.starts_with("심볼릭 링크"), "{off}");
    assert!(off.contains("개발자 모드"), "{off}");
    assert_eq!(words::link_entry(Symlink, true, Os::Windows), "심볼릭 링크");
    assert_eq!(words::link_entry(Shortcut, true, Os::Windows), "바로 가기");
    assert_eq!(words::link_entry(Shortcut, true, Os::MacOs), "별칭");
    assert_eq!(
        words::link_entry(LinkKind::Junction, true, Os::Windows),
        "정션"
    );
}

#[test]
fn no_item_and_items_off_the_local_disk_are_offered_no_link() {
    assert_eq!(link_facts(Os::Windows, &[], false), LinkFacts::default());
    let remote = VPath::new(Scheme::Remote, "p", &["a.txt"]).expect("a remote path");
    let entry = file("a.txt");
    let items = [(remote, &entry)];
    assert_eq!(link_facts(Os::Windows, &items, false), LinkFacts::default());
}

/// The left pane lists `docs`, `x.txt` and `src`; the row menu opens on `row`.
fn menu_on(row: usize) -> AppState {
    let mut state = two_panes();
    state.row_menu(LEFT, row);
    state
}

/// The first entry of the menu: the kind and the link name of `name` it gives.
fn first_entry(state: &AppState, name: &str) -> (LinkKind, std::ffi::OsString) {
    let LinkChoice { kind, enabled } = state.item_links().choice(0).expect("an entry");
    assert!(enabled, "the first entry of every system works");
    (
        kind,
        link_name(std::ffi::OsStr::new(name), kind, Os::CURRENT),
    )
}

#[test]
fn the_menu_opens_with_what_the_items_take() {
    let state = menu_on(2);
    let kinds: Vec<LinkKind> = state.item_links().choices().map(|c| c.kind).collect();
    assert_eq!(kinds, kinds_for(Os::CURRENT, false));
    let state = menu_on(0);
    let kinds: Vec<LinkKind> = state.item_links().choices().map(|c| c.kind).collect();
    assert_eq!(kinds, kinds_for(Os::CURRENT, true));
    // The part of a list without rows has none.
    let mut state = two_panes();
    assert_eq!(state.item_links(), LinkFacts::default());
    state.context_menu(LEFT);
    assert_eq!(state.item_links(), LinkFacts::default());
}

#[test]
fn choosing_a_kind_makes_the_link_in_the_folder_the_pane_shows() {
    let mut state = menu_on(2);
    let (kind, name) = first_entry(&state, "x.txt");
    let (spec, origin) = the_submit(&state.make_links(0, false));
    assert_eq!(origin, Some(LEFT));
    assert_eq!(
        spec,
        JobSpec::MakeLink {
            links: vec![(in_a("x.txt"), folder("a").join(&name).unwrap(), kind)]
        }
    );
    // An entry the menu does not have makes nothing.
    assert_eq!(state.make_links(9, false), []);
}

#[test]
fn a_selection_gets_a_link_to_each_item() {
    let mut state = two_panes();
    state.click_row(LEFT, 0, false, false);
    state.click_row(LEFT, 2, true, false);
    state.row_menu(LEFT, 2);
    let (kind, _) = first_entry(&state, "x.txt");
    let (spec, _) = the_submit(&state.make_links(0, false));
    let JobSpec::MakeLink { links } = spec else {
        panic!("links: {spec:?}");
    };
    let targets: Vec<&VPath> = links.iter().map(|(target, _, _)| target).collect();
    assert_eq!(targets, [&in_a("docs"), &in_a("x.txt")]);
    assert!(
        links
            .iter()
            .all(|(_, at, k)| *k == kind && at.parent() == Some(folder("a")))
    );
}

#[test]
fn an_entry_that_is_off_says_why_and_makes_nothing() {
    let mut state = menu_on(2);
    // The menu as Windows shows it without the rights for a symbolic link.
    let items = [(in_a("x.txt"), &file("x.txt"))];
    state.shell.links = link_facts(Os::Windows, &items, true);
    let off = state
        .item_links()
        .choices()
        .position(|choice| !choice.enabled)
        .expect("one is off");
    assert_eq!(
        state.make_links(off, false),
        [Effect::Notice(words::link_needs_privilege().to_owned())]
    );
    assert_eq!(
        state.make_links(off, true),
        [Effect::Notice(words::link_needs_privilege().to_owned())]
    );
}

fn picker(state: &AppState) -> &Picker {
    match state.modal() {
        Some(Modal::Pick(picker)) => picker,
        other => panic!("the picker shows: {other:?}"),
    }
}

#[test]
fn with_shift_the_picker_finds_the_folder_and_the_links_go_there() {
    let mut state = menu_on(2);
    let (kind, name) = first_entry(&state, "x.txt");
    let effects = state.make_links(0, true);
    // No job yet: the picker shows, for links, with the other pane's folder chosen.
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Job(_))),
        "{effects:?}"
    );
    let shown = picker(&state);
    assert_eq!(shown.purpose, PickPurpose::Link { kind });
    assert!(shown.without_options() && shown.for_links() && !shown.for_a_log());
    assert_eq!(shown.target, Some(folder("b")));
    assert_eq!(shown.request.file_bytes, 0);
    // Its toggle and policy do nothing for links.
    state.pick_mode(TransferMode::Move);
    state.pick_policy(rf_jobs::ConflictPolicy::Overwrite);
    assert_eq!(picker(&state).request.mode, TransferMode::Copy);
    assert_eq!(picker(&state).policy, rf_jobs::ConflictPolicy::Ask);
    // The worker's look at the target tells it is a folder; confirming makes the links there.
    let look = effects.iter().find_map(|effect| match effect {
        Effect::InspectTarget {
            generation, path, ..
        } => Some((*generation, path.clone())),
        _ => None,
    });
    let (generation, path) = look.expect("a look at the target");
    let facts = TargetFacts {
        folder: Ok(()),
        writable: true,
        available: None,
        same_volume: true,
    };
    assert_eq!(state.target_inspected(generation, &path, facts), []);
    let (spec, origin) = the_submit(&state.pick_confirmed());
    assert_eq!(origin, Some(LEFT));
    assert_eq!(
        spec,
        JobSpec::MakeLink {
            links: vec![(in_a("x.txt"), folder("b").join(&name).unwrap(), kind)]
        }
    );
    assert_eq!(state.modal(), None);
    assert_eq!(state.recent_targets().first(), Some(&folder("b")));
}

#[test]
fn the_picker_for_links_cannot_choose_a_folder_inside_the_item() {
    let mut state = menu_on(0);
    state.make_links(0, true);
    let shown = picker(&state);
    // The folder to link is `docs`; a folder inside it is not a place for its link.
    let request = shown.request.clone();
    assert_eq!(
        crate::pick::validate(&request, &in_a("docs").join("deep").unwrap(), None),
        crate::pick::Validity::IntoSource
    );
    assert_eq!(
        words::pick_link_problem(crate::pick::Validity::IntoSource),
        "링크를 만드는 항목 안이라 고를 수 없음"
    );
    // The folder the items are in is where a link goes by default: it can be chosen.
    assert_eq!(
        crate::pick::validate(&request, &folder("a"), None),
        crate::pick::Validity::Ok
    );
}

#[test]
fn a_drop_with_the_keys_for_a_link_makes_the_links_of_the_system() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    state.modifiers_changed(link_keys());
    let offer = state
        .drop_offer(id, RIGHT_LIST)
        .expect("the folder takes it");
    assert_eq!(offer.choice, DropChoice::Do(DropEffect::Link));
    let hover = state.drop_hover(id, RIGHT_LIST);
    assert_eq!(hover.effect(Effects::ALL), Some(DropEffect::Link));
    let (spec, origin) = the_submit(&state.drop_items(id, RIGHT_LIST));
    assert_eq!(origin, Some(LEFT));
    let kind = drop_kind(Os::CURRENT);
    assert_eq!(
        spec,
        JobSpec::MakeLink {
            links: vec![(
                in_a("x.txt"),
                folder("b")
                    .join(link_name(std::ffi::OsStr::new("x.txt"), kind, Os::CURRENT))
                    .unwrap(),
                kind
            )]
        }
    );
    assert_eq!(
        dropped_effect(&state_effects_of(&spec)),
        Some(DropEffect::Link)
    );
}

/// The effects of submitting `spec`.
fn state_effects_of(spec: &JobSpec) -> Vec<Effect> {
    vec![Effect::Job(JobEffect::Submit {
        spec: spec.clone(),
        origin: Some(LEFT),
    })]
}

#[test]
fn the_badge_of_a_drop_with_the_keys_for_a_link_says_so() {
    assert_eq!(
        words::drop_badge(DropChoice::Do(DropEffect::Link), "백업"),
        "링크 만들기: 백업"
    );
    // The old words are gone.
    assert!(!words::drop_badge(DropChoice::Do(DropEffect::Link), "백업").contains("아직"));
}

#[test]
fn a_drag_of_another_app_that_allows_no_link_makes_none() {
    let mut state = two_panes();
    let dragged = DroppedItems {
        paths: vec![std::env::temp_dir().join("c").join("z.txt")],
        ..DroppedItems::default()
    };
    let keys = link_keys();
    // It allows a copy and a move: the keys for a link ask for what it does not allow.
    let over = state.outside_over(
        Some(RIGHT_LIST),
        &dragged,
        keys,
        Effects::COPY | Effects::MOVE,
    );
    assert_eq!(over, None);
    assert_eq!(
        state.outside_drop(
            Some(RIGHT_LIST),
            dragged.clone(),
            keys,
            Effects::COPY | Effects::MOVE,
            false
        ),
        (None, vec![])
    );
    // One that allows a link gets the links, and the system is told a link was made.
    let allowed = Effects::COPY | Effects::LINK;
    assert_eq!(
        state.outside_over(Some(RIGHT_LIST), &dragged, keys, allowed),
        Some(DropEffect::Link)
    );
    let (done, effects) = state.outside_drop(Some(RIGHT_LIST), dragged, keys, allowed, false);
    assert_eq!(done, Some(DropEffect::Link));
    let (spec, _) = the_submit(&effects);
    let JobSpec::MakeLink { links } = spec else {
        panic!("links: {spec:?}");
    };
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].2, drop_kind(Os::CURRENT));
    assert_eq!(links[0].1.parent(), Some(folder("b")));
}

#[test]
fn the_menu_of_a_drop_that_asks_makes_links_where_the_drag_allows_them() {
    let mut state = two_panes();
    state.press_row(LEFT, 2, false, false);
    let id = state.drag_id();
    // With the right button: the menu, which offers the link too.
    let dragged = DroppedItems {
        ours: Some(id),
        ..DroppedItems::default()
    };
    let (done, effects) = state.outside_drop(
        Some(RIGHT_LIST),
        dragged,
        DragMods::default(),
        Effects::ALL,
        true,
    );
    assert_eq!((done, effects), (None, vec![ask_menu(true, true, true)]));
    let (spec, origin) = the_submit(&state.drop_menu_link());
    assert_eq!(origin, Some(LEFT));
    let JobSpec::MakeLink { links } = spec else {
        panic!("links");
    };
    assert_eq!(links[0].0, in_a("x.txt"));
    assert_eq!(links[0].1.parent(), Some(folder("b")));
    // Answered once.
    assert_eq!(state.drop_menu_link(), []);

    // A drag that allows no link has no such entry, and the entry makes nothing.
    let mut state = two_panes();
    let dragged = DroppedItems {
        paths: vec![std::env::temp_dir().join("c").join("z.txt")],
        ..DroppedItems::default()
    };
    let (_, effects) = state.outside_drop(
        Some(RIGHT_LIST),
        dragged,
        DragMods::default(),
        Effects::COPY | Effects::MOVE,
        true,
    );
    assert_eq!(effects, [ask_menu(true, true, false)]);
    assert_eq!(state.drop_menu_link(), []);
}

#[test]
fn the_list_of_favorites_still_makes_no_links() {
    let mut state = two_panes();
    state.press_row(LEFT, 0, false, false);
    let id = state.drag_id();
    state.modifiers_changed(link_keys());
    let between = DropSpot::Favorites { index: Some(0) };
    assert_eq!(state.drop_hover(id, between), Hover::NoKeep(Unkept::Link));
    assert_eq!(state.drop_items(id, between), []);
    assert_eq!(
        words::drop_favorites_no_links(),
        "링크 키를 떼고 놓으면 폴더를 즐겨찾기에 추가합니다"
    );
}

#[test]
fn the_new_links_are_selected_and_the_toast_offers_their_undo() {
    // The folder lists the links the job made, as its watch shows them.
    let kind = kinds_for(Os::CURRENT, false)[0];
    // (Under the name the job gave it after a clash: the link and its target are two rows.)
    let made = std::ffi::OsString::from("x.txt (2)");
    let made_name = made.to_str().expect("text").to_owned();
    let mut state = ready(
        vec![dir("docs"), file("x.txt"), file(&made_name)],
        vec![dir("백업")],
    );
    let spec = link_job(Os::CURRENT, kind, &[in_a("x.txt")], &folder("a")).unwrap();
    let id = JobId(7);
    state.track(Started {
        id,
        spec: Some(Arc::new(spec.clone())),
        work: JobWork::Link,
        origin: Some(LEFT),
        via_history: false,
    });
    let at = folder("a").join(&made).unwrap();
    let record = UndoRecord::LinksMade {
        at: vec![(at.clone(), kind, in_a("x.txt"))],
    };
    let mut report = JobReport::new(id, &spec);
    report.outputs = vec![at];
    report.undo = Some(record.clone());
    report.done.files = 1;
    let effects = state.job_event(rf_jobs::JobEvent::Finished {
        id,
        report: Arc::new(report),
    });
    assert_eq!(selected(&state, LEFT), [2]);
    let shown = toasts(&effects);
    let [(_, text, action, _)] = shown.as_slice() else {
        panic!("one toast: {effects:?}");
    };
    assert_eq!(
        (text.as_str(), action.as_str()),
        ("링크 1개를 만들었습니다", "되돌리기")
    );
    assert_eq!(
        words::links_made(3, 1),
        "링크 3개를 만들었습니다 · 대상이 없는 링크 1개"
    );
}

#[test]
fn one_link_that_fails_says_why_in_the_notice() {
    let kind = LinkKind::Symlink;
    let mut state = ready(vec![file("x.txt")], vec![]);
    let spec = link_job(Os::CURRENT, kind, &[in_a("x.txt")], &folder("a")).unwrap();
    let id = JobId(8);
    state.track(Started {
        id,
        spec: Some(Arc::new(spec.clone())),
        work: JobWork::Link,
        origin: Some(LEFT),
        via_history: false,
    });
    let mut report = JobReport::new(id, &spec);
    report.outcome = Outcome::CompletedWithErrors;
    report.failures.push(rf_jobs::ItemFailure {
        source: Some(in_a("x.txt")),
        target: Some(in_a("x.txt")),
        stage: rf_jobs::Stage::Create,
        kind: ErrorKind::Unsupported,
        message: "needs rights".to_owned(),
        reason: Some(rf_jobs::FailureReason::LinkNeedsPrivilege),
    });
    let effects = state.job_event(rf_jobs::JobEvent::Finished {
        id,
        report: Arc::new(report),
    });
    let notices: Vec<&String> = effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Notice(text) => Some(text),
            _ => None,
        })
        .collect();
    let [text] = notices.as_slice() else {
        panic!("one notice: {effects:?}");
    };
    assert!(text.starts_with("링크를 만들지 못했습니다: "), "{text}");
    assert!(text.contains("개발자 모드"), "{text}");
}

#[test]
fn a_link_job_names_each_link_as_its_system_does() {
    let targets = [in_a("보고서.docx")];
    let at = |os, kind| match link_job(os, kind, &targets, &folder("b")).unwrap() {
        JobSpec::MakeLink { links } => links[0].1.clone(),
        other => panic!("links: {other:?}"),
    };
    let name = |os, kind| at(os, kind).file_name().unwrap().to_owned();
    assert_eq!(
        name(Os::Windows, LinkKind::Shortcut),
        "보고서.docx - 바로 가기.lnk"
    );
    assert_eq!(name(Os::MacOs, LinkKind::Shortcut), "보고서.docx 별칭");
    assert_eq!(name(Os::Windows, LinkKind::Symlink), "보고서.docx");
    // A volume root has no name to give a link.
    let root = VPath::local(if cfg!(windows) { "C:\\" } else { "/" }).unwrap();
    assert!(link_job(Os::CURRENT, LinkKind::Symlink, &[root], &folder("b")).is_err());
}
