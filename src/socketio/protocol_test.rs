use super::{split_id, websocket_url};

#[test]
fn maps_schemes() {
    assert_eq!(
        websocket_url("https://openbmclapi.bangbang93.com"),
        "wss://openbmclapi.bangbang93.com/socket.io/?EIO=4&transport=websocket"
    );
    assert_eq!(
        websocket_url("http://localhost:4000/"),
        "ws://localhost:4000/socket.io/?EIO=4&transport=websocket"
    );
}

#[test]
fn splits_ack_ids() {
    assert_eq!(split_id(r#"["a",1]"#), (None, r#"["a",1]"#));
    assert_eq!(split_id(r#"7["a",1]"#), (Some(7), r#"["a",1]"#));
    assert_eq!(split_id("/admin,12[1]"), (Some(12), "[1]"));
}
