use super::parse_launcher_argument;

#[test]
fn parses_optional_launcher_name() {
    assert_eq!(
        parse_launcher_argument(&["--launcher=totem".into()]),
        Ok(Some("totem".into()))
    );
}

#[test]
fn rejects_empty_and_duplicate_launcher_arguments() {
    assert!(parse_launcher_argument(&["--launcher=".into()]).is_err());
    assert!(
        parse_launcher_argument(&["--launcher=totem".into(), "--launcher=vlc".into()]).is_err()
    );
    assert!(parse_launcher_argument(&["--app=totem".into()]).is_err());
}
