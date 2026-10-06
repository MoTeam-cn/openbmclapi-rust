use super::*;

#[test]
fn picks_ssl_branch() {
    let template = "a<% if (ssl) { %>TLS<% } else { %>PLAIN<% } %>b";
    assert_eq!(resolve_conditionals(template, true), "aTLSb");
    assert_eq!(resolve_conditionals(template, false), "aPLAINb");
}

#[test]
fn substitutes_variables() {
    let mut vars = HashMap::new();
    vars.insert("port", "4000".to_string());
    assert_eq!(render("listen <%= port %>;", &vars, false), "listen 4000;");
}
