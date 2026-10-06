use super::{usage, DiskUsage};

#[test]
fn reports_a_plausible_usage_for_the_working_directory() {
    let path = std::env::current_dir().expect("a working directory");
    let usage = usage(&path).expect("the working directory exists");
    assert!(usage.total > 0, "a real filesystem has a size");
    assert!(
        usage.free <= usage.total,
        "free space cannot exceed the filesystem size"
    );
}

#[test]
fn free_ratio_stays_within_range() {
    let quarter = DiskUsage {
        free: 25,
        total: 100,
    };
    assert!((quarter.free_ratio() - 0.25).abs() < f64::EPSILON);
    let empty = DiskUsage { free: 0, total: 0 };
    assert_eq!(empty.free_ratio(), 0.0, "an unknown size is not a panic");
}

#[test]
fn a_missing_path_is_an_error() {
    let missing = std::env::current_dir()
        .expect("a working directory")
        .join("no-such-directory-4f1a9c");
    assert!(usage(&missing).is_err(), "a missing path cannot be probed");
}
