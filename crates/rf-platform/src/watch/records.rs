//! The record format of `ReadDirectoryChangesW`, parsed on every OS so that tests run everywhere.

use std::ffi::OsString;

use super::DirEvent;

/// `FILE_ACTION_RENAMED_OLD_NAME`: the name before a rename.
const RENAMED_OLD_NAME: u32 = 4;
/// `FILE_ACTION_RENAMED_NEW_NAME`: the name after a rename.
const RENAMED_NEW_NAME: u32 = 5;
/// Bytes of a `FILE_NOTIFY_INFORMATION` before the name: `NextEntryOffset`, `Action` and
/// `FileNameLength`, three `u32`.
const HEADER_LEN: usize = 12;

fn le_u32(header: &[u8; HEADER_LEN], at: usize) -> u32 {
    u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
}

/// The `(action, name)` pairs of a buffer that `ReadDirectoryChangesW` filled with a chain of
/// `FILE_NOTIFY_INFORMATION` records. The name is UTF-16 without a terminator.
///
/// Parsing stops at a record whose header or name does not fit in `buf`, so a cut-off buffer
/// gives the records before the cut and never reads past the end.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by the Windows backend; tested on every OS")
)]
pub(crate) fn parse_notify_records(buf: &[u8]) -> Vec<(u32, Vec<u16>)> {
    let mut records = Vec::new();
    let mut rest = buf;
    while let Some(header) = rest.first_chunk::<HEADER_LEN>() {
        let (next, action, name_len) = (le_u32(header, 0), le_u32(header, 4), le_u32(header, 8));
        let Some(name) = usize::try_from(name_len)
            .ok()
            .and_then(|len| rest.get(HEADER_LEN..HEADER_LEN.checked_add(len)?))
        else {
            break;
        };
        records.push((
            action,
            name.chunks_exact(2)
                .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
                .collect(),
        ));
        // A zero offset marks the last record; one that points past the buffer ends the chain.
        let tail = usize::try_from(next)
            .ok()
            .filter(|&step| step > 0)
            .and_then(|step| rest.get(step..));
        let Some(tail) = tail else {
            break;
        };
        rest = tail;
    }
    records
}

/// Turns the `(action, name)` pairs of one buffer into events: an old name directly followed by
/// a new name is one `Renamed`; everything else, an unpaired half of a rename too, is `Changed`.
#[cfg_attr(
    not(windows),
    allow(dead_code, reason = "used by the Windows backend; tested on every OS")
)]
pub(crate) fn pair_renames(records: Vec<(u32, OsString)>) -> Vec<DirEvent> {
    let mut events = Vec::with_capacity(records.len());
    let mut records = records.into_iter().peekable();
    while let Some((action, name)) = records.next() {
        let new_name = if action == RENAMED_OLD_NAME {
            records.next_if(|(next, _)| *next == RENAMED_NEW_NAME)
        } else {
            None
        };
        events.push(match new_name {
            Some((_, to)) => DirEvent::Renamed { from: name, to },
            None => DirEvent::Changed(name),
        });
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(next: u32, action: u32, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let mut v = Vec::new();
        v.extend(next.to_le_bytes());
        v.extend(action.to_le_bytes());
        v.extend(u32::try_from(units.len() * 2).unwrap().to_le_bytes());
        for u in &units {
            v.extend(u.to_le_bytes());
        }
        v
    }

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn parses_records_and_stops_at_truncation() {
        let mut first = record(0, 1, "a.txt");
        first.resize(first.len().next_multiple_of(4), 0);
        let next = u32::try_from(first.len()).unwrap();
        first[0..4].copy_from_slice(&next.to_le_bytes());
        let buf = [first, record(0, 2, "한글")].concat();
        assert_eq!(
            parse_notify_records(&buf),
            vec![(1, w("a.txt")), (2, w("한글"))]
        );
        assert_eq!(
            parse_notify_records(&buf[..buf.len() - 1]),
            vec![(1, w("a.txt"))]
        );
        assert_eq!(parse_notify_records(&[]), Vec::<(u32, Vec<u16>)>::new());
    }

    #[test]
    fn pairs_renames() {
        let o = OsString::from;
        assert_eq!(
            pair_renames(vec![(4, o("a")), (5, o("b")), (1, o("c"))]),
            vec![
                DirEvent::Renamed {
                    from: o("a"),
                    to: o("b")
                },
                DirEvent::Changed(o("c"))
            ]
        );
        assert_eq!(
            pair_renames(vec![(4, o("a")), (1, o("c"))]),
            vec![DirEvent::Changed(o("a")), DirEvent::Changed(o("c"))]
        );
        assert_eq!(
            pair_renames(vec![(5, o("b"))]),
            vec![DirEvent::Changed(o("b"))]
        );
    }
}
