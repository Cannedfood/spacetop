use super::parse_app_argument;

#[test]
fn parses_optional_app_name() {
    assert_eq!(
        parse_app_argument(&["--app=totem".into()]),
        Ok(Some("totem".into()))
    );
}

#[test]
fn rejects_empty_and_duplicate_app_arguments() {
    assert!(parse_app_argument(&["--app=".into()]).is_err());
    assert!(parse_app_argument(&["--app=totem".into(), "--app=vlc".into()]).is_err());
}
