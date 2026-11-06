use rf_core::{Error, ErrorKind, Scheme, VPath};
use rf_platform::{open_regular_file, rename_no_replace, sync_dir, sync_file};

fn setup() -> (tempfile::TempDir, VPath) {
    let t = tempfile::tempdir().unwrap();
    let v = VPath::local(t.path()).unwrap();
    (t, v)
}

fn write(v: &VPath, s: &str) {
    std::fs::write(v.as_local().unwrap(), s).unwrap();
}

fn read(v: &VPath) -> String {
    std::fs::read_to_string(v.as_local().unwrap()).unwrap()
}

#[test]
fn renames_to_a_free_name() {
    let (_t, d) = setup();
    let (a, b) = (d.join("a").unwrap(), d.join("b").unwrap());
    write(&a, "A");
    rename_no_replace(&a, &b).unwrap();
    assert!(!a.as_local().unwrap().exists());
    assert_eq!(read(&b), "A");
}

#[test]
fn refuses_to_replace_a_file() {
    let (_t, d) = setup();
    let (a, b) = (d.join("a").unwrap(), d.join("b").unwrap());
    write(&a, "A");
    write(&b, "B");
    assert_eq!(
        rename_no_replace(&a, &b).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!((read(&a), read(&b)), (String::from("A"), String::from("B")));
}

#[test]
fn refuses_to_replace_a_directory() {
    let (_t, d) = setup();
    let (src, dst) = (d.join("src").unwrap(), d.join("dst").unwrap());
    std::fs::create_dir(src.as_local().unwrap()).unwrap();
    std::fs::create_dir(dst.as_local().unwrap()).unwrap();
    write(&src.join("keep").unwrap(), "k");
    assert_eq!(
        rename_no_replace(&src, &dst).unwrap_err().kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(read(&src.join("keep").unwrap()), "k");
    assert_eq!(
        std::fs::read_dir(dst.as_local().unwrap()).unwrap().count(),
        0
    );
}

#[test]
fn missing_source_is_not_found() {
    let (_t, d) = setup();
    let err = rename_no_replace(&d.join("missing").unwrap(), &d.join("x").unwrap()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn moves_into_another_directory() {
    let (_t, d) = setup();
    let (a, sub) = (d.join("a").unwrap(), d.join("sub").unwrap());
    write(&a, "A");
    std::fs::create_dir(sub.as_local().unwrap()).unwrap();
    rename_no_replace(&a, &sub.join("a").unwrap()).unwrap();
    assert_eq!(read(&sub.join("a").unwrap()), "A");
}

#[test]
fn non_local_is_unsupported() {
    let (_t, d) = setup();
    let remote = VPath::new(Scheme::Remote, "p", &["x"]).unwrap();
    assert_eq!(
        rename_no_replace(&remote, &d).unwrap_err().kind(),
        ErrorKind::Unsupported
    );
}

#[test]
fn error_names_the_source_and_the_operation() {
    let (_t, d) = setup();
    let (a, b) = (d.join("a").unwrap(), d.join("b").unwrap());
    write(&a, "A");
    write(&b, "B");
    let Error::Io { op, path, .. } = rename_no_replace(&a, &b).unwrap_err() else {
        panic!("expected an I/O error");
    };
    assert_eq!((op, path), ("rename", a));
}

/// Needs a second volume; `/dev/shm` is one on Linux, and the test skips where there is none.
#[cfg(unix)]
#[test]
fn another_volume_crosses_devices() {
    use std::os::unix::fs::MetadataExt;

    let (_t, d) = setup();
    let Ok(other) = tempfile::tempdir_in("/dev/shm") else {
        eprintln!("skipped: no /dev/shm");
        return;
    };
    let device = |p: &std::path::Path| std::fs::metadata(p).unwrap().dev();
    if device(other.path()) == device(d.as_local().unwrap()) {
        eprintln!("skipped: /dev/shm is on the same volume");
        return;
    }
    let a = d.join("a").unwrap();
    let far = VPath::local(other.path()).unwrap().join("a").unwrap();
    write(&a, "A");
    assert_eq!(
        rename_no_replace(&a, &far).unwrap_err().kind(),
        ErrorKind::CrossesDevices
    );
    assert_eq!(read(&a), "A");
    assert!(!far.as_local().unwrap().exists());
}

/// A file and a folder with new entries are made durable; a folder that is missing is an error
/// on every system, never a silent success.
#[test]
fn files_and_folders_are_synced() {
    let (t, d) = setup();
    let a = d.join("a").unwrap();
    write(&a, "A");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(a.as_local().unwrap())
        .unwrap();
    sync_file(&file).unwrap();
    let native = d.to_extended_length().unwrap();
    sync_dir(&native).unwrap();
    let missing = sync_dir(&native.join("missing")).unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    // A file is no folder to sync.
    assert!(sync_dir(&t.path().join("a")).is_err());
}

/// A regular file opens and reads from its start; a folder is refused.
#[test]
fn only_a_regular_file_opens_for_reading() {
    use std::io::Read;

    let (t, d) = setup();
    write(&d.join("a").unwrap(), "content");
    let mut file = open_regular_file(&t.path().join("a")).unwrap();
    let mut read = String::new();
    file.read_to_string(&mut read).unwrap();
    assert_eq!(read, "content");
    let missing = open_regular_file(&t.path().join("missing")).unwrap_err();
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    assert!(open_regular_file(t.path()).is_err());
}

/// The end of a FIFO with no writer would block a plain open for good; both the open for
/// reading and the folder sync refuse it at once instead.
#[cfg(unix)]
#[test]
fn a_fifo_never_blocks_an_open() {
    use std::sync::mpsc;
    use std::time::Duration;

    let t = tempfile::tempdir().unwrap();
    let fifo = t.path().join("fifo");
    // `mkfifo(1)` is on Linux and macOS alike; rustix has no `mkfifoat` on macOS.
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    assert!(made.unwrap().success(), "mkfifo failed");
    let (tx, rx) = mpsc::channel();
    let path = fifo.clone();
    std::thread::spawn(move || {
        let read = open_regular_file(&path).map(drop);
        let synced = sync_dir(&path);
        let _ = tx.send((read, synced));
    });
    let Ok((read, synced)) = rx.recv_timeout(Duration::from_secs(10)) else {
        // Opening the other end lets the blocked open go on, so the thread ends.
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
        panic!("an open of a FIFO blocked");
    };
    let read = read.unwrap_err();
    assert_eq!(read.kind(), std::io::ErrorKind::Unsupported, "{read}");
    assert_eq!(read.to_string(), "not a regular file");
    assert!(rf_platform::is_not_a_regular_file(&read));
    assert!(synced.is_err());
}

/// Holds a write lease on a file, as Samba with kernel oplocks or the NFS server's delegations
/// do: `python3` writes `leased` to the file, takes the lease (`F_SETLEASE`, 1024) and says
/// `held`. When an open of the file starts the lease break (`SIGIO`), it lets go after the
/// number of seconds it was given, or never with `never`. It ends after 10 s in any case.
#[cfg(target_os = "linux")]
const LEASE_HOLDER: &str = r#"
import fcntl, os, signal, sys, time
path, delay = sys.argv[1], sys.argv[2]
with open(path, "w") as f:
    f.write("leased")
broken = []
signal.signal(signal.SIGIO, lambda *_: broken.append(time.monotonic()))
fd = os.open(path, os.O_RDONLY)
try:
    fcntl.fcntl(fd, 1024, fcntl.F_WRLCK)
except OSError as e:
    print("unsupported", e, flush=True)
    sys.exit()
print("held", flush=True)
end = time.monotonic() + 10
while time.monotonic() < end:
    if broken and delay != "never" and time.monotonic() >= broken[0] + float(delay):
        fcntl.fcntl(fd, 1024, fcntl.F_UNLCK)
        delay = "never"
    time.sleep(0.01)
"#;

/// A `python3` running [`LEASE_HOLDER`]; it is killed when this is dropped.
#[cfg(target_os = "linux")]
struct LeaseHolder(std::process::Child);

#[cfg(target_os = "linux")]
impl Drop for LeaseHolder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts a [`LEASE_HOLDER`] on `path` that lets go `delay` after the break begins, once it
/// holds the lease; without `python3`, or where no lease can be taken, why not.
#[cfg(target_os = "linux")]
fn hold_lease(path: &std::path::Path, delay: &str) -> Result<LeaseHolder, String> {
    use std::io::BufRead;
    use std::process::{Command, Stdio};

    let mut child = Command::new("python3")
        .args(["-c", LEASE_HOLDER])
        .arg(path)
        .arg(delay)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("no python3: {error}"))?;
    let stdout = child.stdout.take().ok_or("no output")?;
    let holder = LeaseHolder(child);
    let mut line = String::new();
    std::io::BufReader::new(stdout)
        .read_line(&mut line)
        .map_err(|error| error.to_string())?;
    if line.trim() == "held" {
        Ok(holder)
    } else {
        Err(format!("no lease here: {line}"))
    }
}

/// A file another program holds a write lease on cannot be opened, not even for reading, until
/// that program lets go: a non-blocking open fails with `EAGAIN` meanwhile. The open waits a
/// while for it, and then fails with a clear `ResourceBusy` error instead of `EAGAIN`.
#[cfg(target_os = "linux")]
#[test]
fn an_open_waits_a_while_for_a_lease_to_be_let_go() {
    use std::io::Read;
    use std::time::{Duration, Instant};

    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("leased.txt");
    let holder = match hold_lease(&path, "0.3") {
        Ok(holder) => holder,
        Err(why) => {
            eprintln!("skipped: {why}");
            return;
        }
    };
    let mut content = String::new();
    let mut file = open_regular_file(&path).unwrap();
    file.read_to_string(&mut content).unwrap();
    assert_eq!(content, "leased");
    drop((file, holder));

    // The kernel breaks a lease itself after `lease-break-time`, 45 s by default.
    let break_time = std::fs::read_to_string("/proc/sys/fs/lease-break-time")
        .ok()
        .and_then(|secs| secs.trim().parse::<u64>().ok());
    if break_time.is_none_or(|secs| secs < 5) {
        eprintln!("skipped: the kernel breaks leases too soon");
        return;
    }
    let holder = match hold_lease(&path, "never") {
        Ok(holder) => holder,
        Err(why) => {
            eprintln!("skipped: {why}");
            return;
        }
    };
    let started = Instant::now();
    let error = open_regular_file(&path).unwrap_err();
    let waited = started.elapsed();
    drop(holder);
    assert_eq!(error.kind(), std::io::ErrorKind::ResourceBusy, "{error}");
    assert!(
        error.to_string().contains("locked by another program"),
        "{error}"
    );
    assert!(
        (Duration::from_millis(1500)..Duration::from_secs(5)).contains(&waited),
        "{waited:?}"
    );
}
