use super::{is_reserved, key, payload, DIR};

#[test]
fn keys_are_named_by_the_bare_size() {
    assert_eq!(key(0), "measure/0");
    assert_eq!(key(10), "measure/10");
    assert_eq!(key(128), "measure/128");
}

#[test]
fn the_reserved_folder_is_recognised_but_nothing_else_is() {
    assert!(is_reserved(DIR));
    assert!(is_reserved("measure/10"));
    assert!(!is_reserved("ab/abcdef"));
    assert!(!is_reserved("measure-other/10"));
    assert!(!is_reserved("ab/measure"));
}

#[test]
fn a_payload_is_exactly_the_requested_size() {
    assert_eq!(payload(1).len(), 1024 * 1024);
    assert_eq!(payload(0).len(), 0, "size zero is a valid placeholder");
}

#[test]
fn a_payload_is_stable_for_a_given_size() {
    assert_eq!(payload(1), payload(1), "nodes must agree byte for byte");
}

#[test]
fn different_sizes_get_different_payloads() {
    let one = payload(1);
    let two = payload(2);
    assert_ne!(one[..64], two[..64], "each probe is seeded by its size");
}

#[test]
fn a_payload_does_not_repeat_in_compressible_runs() {
    let bytes = payload(1);
    let mut longest = 1usize;
    let mut run = 1usize;
    for pair in bytes.windows(2) {
        if pair[0] == pair[1] {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 1;
        }
    }
    assert!(
        longest < 16,
        "a repeating payload would be compressed away and misreport throughput"
    );
}
