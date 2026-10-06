use super::{access_line, app_line, Fields};

fn with_message(message: &str) -> Fields {
    Fields {
        message: message.to_string(),
        named: Vec::new(),
    }
}

fn with_fields(pairs: &[(&str, &str)]) -> Fields {
    Fields {
        message: "request".to_string(),
        named: pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    }
}

#[test]
fn the_application_line_matches_the_node_shape() {
    let line = app_line(
        "2026-08-08 22:33:35.659 +0800",
        "INFO",
        512242,
        &with_message("booting openbmclapi 1.14.0"),
    );
    assert_eq!(
        line,
        "[2026-08-08 22:33:35.659 +0800] INFO (512242): booting openbmclapi 1.14.0\n"
    );
}

#[test]
fn extra_fields_are_appended_to_the_application_line() {
    let line = app_line("t", "WARN", 1, &with_fields(&[("path", "/download/seed")]));
    assert_eq!(line, "[t] WARN (1): request path=/download/seed\n");
}

#[test]
fn the_access_line_matches_morgans_combined_format() {
    let line = access_line(
        "08/Aug/2026:14:41:58 +0000",
        &with_fields(&[
            ("remote", "115.231.217.214"),
            ("method", "GET"),
            ("uri", "/download/698abac2?s=abc&e=def"),
            ("version", "1.1"),
            ("status", "302"),
            ("length", "-"),
            ("referer", "-"),
            ("user_agent", "bmclapi-ctrl/3.19.5"),
        ]),
    );
    let expected = "115.231.217.214 - - [08/Aug/2026:14:41:58 +0000] \"GET /download/698abac2?s=abc&e=def HTTP/1.1\" 302 - \"-\" \"bmclapi-ctrl/3.19.5\"\n";
    assert_eq!(line, expected);
}

#[test]
fn missing_access_fields_render_as_dashes() {
    let line = access_line("t", &with_message("request"));
    assert_eq!(line, "- - - [t] \"- - HTTP/-\" - - \"-\" \"-\"\n");
}
