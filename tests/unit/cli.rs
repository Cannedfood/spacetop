use super::parse_launcher_argument;
use spacetop_config::AppConfig;

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
    assert!(parse_launcher_argument(&["--xr-client".into()]).is_err());
}

#[test]
fn configured_launcher_is_kept_without_override() {
    let mut config = AppConfig::default();
    config.application.launcher = "configured-launcher".into();

    let args: &[String] = &[];
    if let Some(program) = parse_launcher_argument(args).unwrap() {
        config.application.launcher = program;
    }

    assert_eq!(config.application.launcher, "configured-launcher");
}
