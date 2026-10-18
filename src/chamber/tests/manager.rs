use super::parse_chamber_file;

#[test]
fn parses_chamber_and_job_fields() {
    let source = r#"
        [chamber]
        name = "nightly"
        schema_version = 1

        [[jobs]]
        name = "backup"
        enabled = false
        cron = "0 0 * * *"
        working_directory = "."
        command = "./backup.sh"
        timeout = "30m"
    "#;

    let config = parse_chamber_file(source).expect("valid chamber config should parse");

    assert_eq!(config.chamber.name, "nightly");
    assert_eq!(config.chamber.schema_version, 1);
    assert_eq!(config.jobs.len(), 1);
    assert_eq!(config.jobs[0].name, "backup");
    assert!(!config.jobs[0].enabled);
    assert_eq!(config.jobs[0].cron, "0 0 * * *");
    assert_eq!(config.jobs[0].working_directory, ".");
    assert_eq!(config.jobs[0].command, "./backup.sh");
    assert_eq!(config.jobs[0].timeout, "30m");
}

#[test]
fn applies_defaults_for_jobs_and_enabled() {
    let without_jobs = r#"
        [chamber]
        name = "empty"
        schema_version = 1
    "#;

    let config = parse_chamber_file(without_jobs).expect("jobs should default to an empty list");
    assert!(config.jobs.is_empty());

    let with_job = r#"
        [chamber]
        name = "daily"
        schema_version = 1

        [[jobs]]
        name = "cleanup"
        cron = "0 0 * * *"
        working_directory = "."
        command = "./cleanup.sh"
        timeout = "5m"
    "#;

    let config = parse_chamber_file(with_job).expect("enabled should default to true");
    assert!(config.jobs[0].enabled);
}

#[test]
fn rejects_invalid_toml_and_unknown_fields() {
    let invalid_toml = "[chamber\nname = \"broken\"";
    assert!(parse_chamber_file(invalid_toml).is_err());

    let unknown_field = r#"
        [chamber]
        name = "unknown"
        schema_version = 1
        unexpected = true
    "#;
    assert!(parse_chamber_file(unknown_field).is_err());
}
