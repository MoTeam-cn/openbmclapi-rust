use super::{bar, file_line, overall_line, ratio, FileBar, Progress, BAR};

#[test]
fn the_bar_is_one_space_plus_forty_cells() {
    assert_eq!(bar(0.0).chars().count(), BAR + 1);
    assert_eq!(bar(1.0).chars().count(), BAR + 1);
}

#[test]
fn the_bar_fills_proportionally() {
    assert_eq!(bar(0.0), " ----------------------------------------");
    assert_eq!(bar(1.0), " ========================================");
    // One file of three, as the Node agent drew it.
    assert_eq!(bar(1.0 / 3.0), " =============---------------------------");
}

#[test]
fn the_overall_line_names_the_file_count() {
    assert_eq!(
        overall_line(1, 3),
        format!("{} | files | 1/3", bar(1.0 / 3.0))
    );
    assert_eq!(overall_line(3, 3), format!("{} | files | 3/3", bar(1.0)));
}

#[test]
fn the_file_line_names_the_object_and_its_bytes() {
    let line = file_line("/maven/x.jar", 764421, 764421);
    assert!(line.ends_with(" | /maven/x.jar | 764421/764421"), "{line}");
}

#[test]
fn nothing_to_do_reads_as_complete() {
    assert_eq!(bar(ratio(0, 0)), bar(1.0));
}

#[test]
fn more_than_the_total_never_overflows_the_bar() {
    assert_eq!(bar(ratio(5, 3)), bar(1.0));
}

#[test]
fn a_disabled_display_accepts_every_call() {
    let progress = Progress::new(3, false);
    assert!(!progress.is_live());
    let bar = progress.start_file("x", 10);
    progress.add_bytes(bar, 5);
    progress.finish_file(bar);
    progress.finish();
    assert_eq!(progress.counters(), (0, 0));
}

#[test]
fn bytes_land_on_the_file_that_reported_them() {
    let progress = Progress::forced(1);
    let first = progress.start_file("a", 100);
    let second = progress.start_file("b", 100);
    progress.add_bytes(first, 40);
    progress.add_bytes(second, 70);
    progress.add_bytes(FileBar(999), 10);
    assert_eq!(progress.done_bytes(first), 40);
    assert_eq!(progress.done_bytes(second), 70);
    assert_eq!(progress.done_bytes(FileBar(999)), 0);
}

#[test]
fn finishing_retires_a_file_and_counts_it() {
    let progress = Progress::forced(2);
    let first = progress.start_file("a", 100);
    let second = progress.start_file("b", 100);
    assert_eq!(progress.counters(), (0, 2));
    progress.finish_file(first);
    assert_eq!(progress.counters(), (1, 1));
    progress.finish_file(second);
    assert_eq!(progress.counters(), (2, 0));
}
