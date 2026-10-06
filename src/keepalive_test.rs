use super::*;

#[test]
fn splits_ack_arrays() {
    let ack = json!([null, true]);
    let (err, value) = split_ack(&ack);
    assert!(err.is_none());
    assert_eq!(value, Some(&serde_json::Value::Bool(true)));

    let ack = json!([{"message": "nope"}, null]);
    let (err, _) = split_ack(&ack);
    assert!(err.is_some());
}
