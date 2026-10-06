#[test]
fn hash_key_uses_slash() {
    assert_eq!(crate::util::hash_to_filename("abcdef"), "ab/abcdef");
}
