use std::cmp::Ordering;
use std::time::{Duration, SystemTime};

use rf_core::{Attributes, Entry, EntryKind};
use rf_vfs::SortKey::{Kind, Modified, Name, Size};
use rf_vfs::SortOrder::{Ascending, Descending};
use rf_vfs::{
    SortBy, SortKey, SortOrder, SortSpec, VisibilityFilter, compare_entries, sort_entries,
};

fn f(name: &str) -> Entry {
    Entry::new(name, EntryKind::File)
}

fn d(name: &str) -> Entry {
    Entry::new(name, EntryKind::Dir)
}

fn sized(name: &str, size: u64) -> Entry {
    let mut x = f(name);
    x.size = size;
    x
}

fn names(v: &[Entry]) -> Vec<String> {
    v.iter()
        .map(|x| x.name.to_string_lossy().into_owned())
        .collect()
}

fn by(key: SortKey, order: SortOrder) -> SortBy {
    SortBy { key, order }
}

fn spec(primary: SortBy, folders_first: bool) -> SortSpec {
    SortSpec {
        primary,
        secondary: None,
        folders_first,
    }
}

fn sorted(mut v: Vec<Entry>, s: SortSpec) -> Vec<String> {
    sort_entries(&mut v, &s);
    names(&v)
}

#[test]
fn default_is_natural_name_order_with_folders_first() {
    assert_eq!(SortSpec::default(), spec(by(Name, Ascending), true));
    let v = vec![
        f("file10.txt"),
        f("file2.txt"),
        f("File1.txt"),
        d("dir B"),
        d("dir a"),
    ];
    assert_eq!(
        sorted(v, SortSpec::default()),
        ["dir a", "dir B", "File1.txt", "file2.txt", "file10.txt"]
    );
    assert_eq!(
        sorted(vec![f("사진10.jpg"), f("사진2.jpg")], SortSpec::default()),
        ["사진2.jpg", "사진10.jpg"]
    );
}

#[test]
fn folders_first_and_direction() {
    let v = || vec![f("a.txt"), d("m-folder"), f("z.txt")];
    assert_eq!(
        sorted(v(), spec(by(Name, Ascending), false)),
        ["a.txt", "m-folder", "z.txt"]
    );
    assert_eq!(
        sorted(v(), spec(by(Name, Ascending), true)),
        ["m-folder", "a.txt", "z.txt"]
    );
    assert_eq!(
        sorted(v(), spec(by(Name, Descending), true)),
        ["m-folder", "z.txt", "a.txt"]
    );
    assert_eq!(
        sorted(v(), spec(by(Name, Descending), false)),
        ["z.txt", "m-folder", "a.txt"]
    );
}

#[test]
fn size_and_modified_orders() {
    let v = || {
        vec![
            sized("c", 30),
            sized("a", 10),
            sized("b", 20),
            sized("e", 5),
            sized("d", 5),
        ]
    };
    assert_eq!(
        sorted(v(), spec(by(Size, Ascending), false)),
        ["d", "e", "a", "b", "c"]
    );
    assert_eq!(
        sorted(v(), spec(by(Size, Descending), false)),
        ["c", "b", "a", "d", "e"]
    );
    let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    let at = |name: &str, time: Option<SystemTime>| {
        let mut entry = f(name);
        entry.modified = time;
        entry
    };
    let timed = || {
        vec![
            at("old", Some(t0)),
            at("new", Some(t0 + Duration::from_secs(60))),
            at("none", None),
        ]
    };
    assert_eq!(
        sorted(timed(), spec(by(Modified, Ascending), false)),
        ["none", "old", "new"]
    );
    assert_eq!(
        sorted(timed(), spec(by(Modified, Descending), false)),
        ["new", "old", "none"]
    );
}

#[test]
fn kind_sort_groups_by_kind_then_extension() {
    let v = vec![
        Entry::new("l", EntryKind::Symlink),
        f("b.txt"),
        f("a.png"),
        f("noext"),
        d("x"),
        f("c.PNG"),
    ];
    assert_eq!(
        sorted(v, spec(by(Kind, Ascending), false)),
        ["x", "noext", "a.png", "c.PNG", "b.txt", "l"]
    );
}

#[test]
fn created_and_extension_orders() {
    let at = |name: &str, secs: Option<u64>| {
        let mut x = f(name);
        x.created = secs.map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        x
    };
    // An unknown creation time (a file system that does not keep one) sorts first.
    let v = || {
        vec![
            at("new", Some(30)),
            at("unknown", None),
            at("old", Some(10)),
        ]
    };
    assert_eq!(
        sorted(v(), spec(by(SortKey::Created, Ascending), true)),
        ["unknown", "old", "new"]
    );
    assert_eq!(
        sorted(v(), spec(by(SortKey::Created, Descending), true)),
        ["new", "old", "unknown"]
    );

    // The extension alone, in any case; no extension (a folder, `noext`) sorts first, and a
    // tie goes by the name. A link is not set apart, as the kind sort sets it.
    let v = vec![
        Entry::new("l.png", EntryKind::Symlink),
        f("b.txt"),
        f("c.PNG"),
        f("noext"),
        d("x"),
        f("a.png"),
    ];
    assert_eq!(
        sorted(v, spec(by(SortKey::Extension, Ascending), false)),
        ["noext", "x", "a.png", "c.PNG", "l.png", "b.txt"]
    );
}

#[test]
fn secondary_key_breaks_ties() {
    let v = vec![
        sized("small.txt", 1),
        sized("z-big.txt", 9),
        sized("a.png", 5),
    ];
    let s = SortSpec {
        primary: by(Kind, Ascending),
        secondary: Some(by(Size, Descending)),
        folders_first: false,
    };
    assert_eq!(sorted(v, s), ["a.png", "z-big.txt", "small.txt"]);
}

#[test]
fn sort_agrees_with_compare_entries() {
    let entries: Vec<Entry> = (0..200u64)
        .map(|i| {
            let id = (i * 37) % 200;
            let mut entry = if i % 7 == 0 {
                d(&format!("item{id}"))
            } else {
                f(&format!("item{id}.t{}", i % 3))
            };
            entry.size = (i * 13) % 50;
            entry.modified =
                (i % 5 != 0).then(|| SystemTime::UNIX_EPOCH + Duration::from_secs(i % 17));
            entry
        })
        .collect();
    let specs = [
        SortSpec::default(),
        spec(by(Size, Descending), true),
        spec(by(Modified, Ascending), false),
        SortSpec {
            primary: by(Kind, Ascending),
            secondary: Some(by(Name, Descending)),
            folders_first: false,
        },
    ];
    for sort_spec in specs {
        let mut ordered = entries.clone();
        sort_entries(&mut ordered, &sort_spec);
        assert_eq!(ordered.len(), entries.len());
        let in_order = ordered
            .windows(2)
            .all(|pair| compare_entries(&pair[0], &pair[1], &sort_spec) != Ordering::Greater);
        assert!(in_order, "{sort_spec:?}");
    }
}

#[test]
fn visibility_filter_follows_explorer_rules() {
    let mk = |n: &str, a: Attributes| {
        let mut x = f(n);
        x.attributes = a;
        x
    };
    let all = vec![
        mk("plain", Attributes::empty()),
        mk("hidden", Attributes::HIDDEN),
        mk("system", Attributes::SYSTEM),
        mk("protected", Attributes::HIDDEN | Attributes::SYSTEM),
    ];
    let shown = |flt: VisibilityFilter| {
        let mut v = all.clone();
        flt.retain(&mut v);
        names(&v)
    };
    assert_eq!(shown(VisibilityFilter::default()), ["plain", "system"]);
    assert_eq!(
        shown(VisibilityFilter {
            show_hidden: true,
            show_system: false
        }),
        ["plain", "hidden", "system"]
    );
    assert_eq!(
        shown(VisibilityFilter {
            show_hidden: true,
            show_system: true
        }),
        ["plain", "hidden", "system", "protected"]
    );
    assert_eq!(
        shown(VisibilityFilter {
            show_hidden: false,
            show_system: true
        }),
        ["plain", "system"]
    );
    assert!(
        VisibilityFilter::default().is_visible(&all[0])
            && !VisibilityFilter::default().is_visible(&all[1])
    );
}
