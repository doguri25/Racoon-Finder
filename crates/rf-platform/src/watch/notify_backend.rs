//! Turns `notify` events into [`DirEvent`]s for inotify (Linux) and `FSEvents` (macOS).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use notify::event::{ModifyKind, RenameMode};
use notify::{Event, EventKind};

use super::DirEvent;

/// The name of `path` when it lies directly inside one of the watched folders `dirs`.
fn child_name(path: &Path, dirs: &[PathBuf]) -> Option<OsString> {
    let parent = path.parent()?;
    if !dirs.iter().any(|dir| dir == parent) {
        return None;
    }
    path.file_name().map(OsString::from)
}

/// `Renamed` for a rename that `notify` reports with both names when both are inside the
/// watched folders.
fn renamed_within(event: &Event, dirs: &[PathBuf]) -> Option<DirEvent> {
    if event.kind != EventKind::Modify(ModifyKind::Name(RenameMode::Both)) {
        return None;
    }
    let [from, to] = event.paths.as_slice() else {
        return None;
    };
    Some(DirEvent::Renamed {
        from: child_name(from, dirs)?,
        to: child_name(to, dirs)?,
    })
}

/// The events one `notify` event stands for in the watched folder, which `dirs` names. The
/// folder can appear under more than one spelling (`/var/x` and `/private/var/x` on macOS),
/// so `dirs` holds them all.
///
/// A change to the folder itself that removes or renames it is `Ended`. Changes to the folder
/// that leave it in place, to anything deeper than its direct children, and to anything
/// elsewhere give nothing. A reading of a file is not a change and gives nothing either.
pub(crate) fn route(event: &Event, dirs: &[PathBuf]) -> Vec<DirEvent> {
    if event.need_rescan() {
        return vec![DirEvent::Overflow];
    }
    if matches!(event.kind, EventKind::Access(_)) {
        return Vec::new();
    }
    if let Some(renamed) = renamed_within(event, dirs) {
        return vec![renamed];
    }
    let ends_folder = matches!(
        event.kind,
        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
    );
    let mut events = Vec::new();
    for path in &event.paths {
        if dirs.contains(path) {
            if ends_folder && !events.contains(&DirEvent::Ended) {
                events.push(DirEvent::Ended);
            }
        } else if let Some(name) = child_name(path, dirs) {
            events.push(DirEvent::Changed(name));
        }
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, Flag, MetadataKind, ModifyKind, RemoveKind, RenameMode,
    };
    use notify::{Event, EventKind};

    fn ev(kind: EventKind, paths: &[&str]) -> Event {
        paths
            .iter()
            .fold(Event::new(kind), |e, p| e.add_path(PathBuf::from(p)))
    }

    fn ch(n: &str) -> DirEvent {
        DirEvent::Changed(n.into())
    }

    #[test]
    fn routes_child_events() {
        let w = [PathBuf::from("/w")];
        assert_eq!(
            route(&ev(EventKind::Create(CreateKind::File), &["/w/a"]), &w),
            [ch("a")]
        );
        let both = EventKind::Modify(ModifyKind::Name(RenameMode::Both));
        assert_eq!(
            route(&ev(both, &["/w/a", "/w/b"]), &w),
            [DirEvent::Renamed {
                from: "a".into(),
                to: "b".into()
            }]
        );
        assert_eq!(route(&ev(both, &["/w/a", "/elsewhere/b"]), &w), [ch("a")]);
        assert_eq!(
            route(&ev(EventKind::Remove(RemoveKind::File), &["/w/a"]), &w),
            [ch("a")]
        );
    }

    #[test]
    fn routes_self_events_to_ended() {
        let w = [PathBuf::from("/w")];
        assert_eq!(
            route(&ev(EventKind::Remove(RemoveKind::Folder), &["/w"]), &w),
            [DirEvent::Ended]
        );
        let from = EventKind::Modify(ModifyKind::Name(RenameMode::From));
        assert_eq!(route(&ev(from, &["/w"]), &w), [DirEvent::Ended]);
        let meta = EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any));
        assert!(route(&ev(meta, &["/w"]), &w).is_empty());
    }

    #[test]
    fn ignores_access_and_nested_events() {
        let w = [PathBuf::from("/w")];
        let open = EventKind::Access(AccessKind::Open(AccessMode::Any));
        assert!(route(&ev(open, &["/w/a"]), &w).is_empty());
        assert!(route(&ev(EventKind::Create(CreateKind::File), &["/w/sub/x"]), &w).is_empty());
        assert!(route(&ev(EventKind::Create(CreateKind::File), &["/other/a"]), &w).is_empty());
    }

    #[test]
    fn maps_rescan_and_canonical_alias() {
        let w = [PathBuf::from("/var/w"), PathBuf::from("/private/var/w")];
        assert_eq!(
            route(&Event::new(EventKind::Other).set_flag(Flag::Rescan), &w),
            [DirEvent::Overflow]
        );
        assert_eq!(
            route(
                &ev(EventKind::Create(CreateKind::File), &["/private/var/w/a"]),
                &w
            ),
            [ch("a")]
        );
    }
}
