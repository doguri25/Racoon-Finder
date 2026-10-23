use rf_core::VPath;
use rf_vfs::{LIST_BATCH_SIZE, LocalProvider, Provider, testkit};

fn run(check: fn(&dyn Provider, &VPath)) {
    let t = tempfile::tempdir().unwrap();
    check(&LocalProvider::new(), &VPath::local(t.path()).unwrap());
}

#[test]
fn create_and_stat() {
    run(testkit::check_create_and_stat);
}

#[test]
fn list_batches_are_full_except_the_last() {
    let t = tempfile::tempdir().unwrap();
    let sizes =
        testkit::check_list_batches(&LocalProvider::new(), &VPath::local(t.path()).unwrap());
    assert_eq!(sizes, [LIST_BATCH_SIZE, LIST_BATCH_SIZE, 503]);
}

#[test]
fn list_cancel() {
    run(testkit::check_list_cancel);
}

#[test]
fn rename() {
    run(testkit::check_rename);
}

#[test]
fn read_write() {
    run(testkit::check_read_write);
}

#[test]
fn delete_permanent() {
    run(testkit::check_delete_permanent);
}
