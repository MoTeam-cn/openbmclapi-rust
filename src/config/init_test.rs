use super::template;
use crate::config::yaml::parse;

#[test]
fn the_init_template_parses_with_this_reader() {
    let parsed = parse(template());
    assert!(parsed.is_ok(), "template must parse: {:?}", parsed.err());
}

#[test]
fn the_init_template_mentions_the_documented_keys() {
    let text = template();
    for key in [
        "cluster_id",
        "cluster_secret",
        "port",
        "storage",
        "sources",
        "log_level",
        "enable_upnp",
    ] {
        assert!(text.contains(key), "template should mention {key}");
    }
}
