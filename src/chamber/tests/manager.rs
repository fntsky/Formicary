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

#[test]
fn loads_only_imported_chamber_without_reloading_existing_configs() {
    let root = std::env::temp_dir().join(format!("formicary-import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("existing")).unwrap();
    std::fs::write(root.join("existing/chamber.toml"), "invalid configuration").unwrap();
    std::fs::create_dir_all(root.join("imported")).unwrap();
    std::fs::write(
        root.join("imported/chamber.toml"),
        "[chamber]\nname = \"imported\"\nschema_version = 1\n",
    )
    .unwrap();
    let manager = super::ChamberManager::load_directory(&root, Some("imported")).unwrap();
    assert_eq!(manager.chambers().len(), 1);
    assert_eq!(manager.chambers()[0].name, "imported");
    std::fs::write(
        root.join("imported/chamber.toml"),
        "[chamber]\nname = \"wrong-name\"\nschema_version = 1\n",
    )
    .unwrap();
    assert!(super::ChamberManager::load_directory(&root, Some("imported")).is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn derives_import_directory_from_configuration_and_rejects_traversal() {
    let root = std::env::temp_dir().join(format!("formicary-import-name-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    for (name, valid) in [
        ("my-chamber", true),
        ("../outside", false),
        ("/absolute", false),
        ("", false),
    ] {
        let source = format!("[chamber]\nname = {name:?}\nschema_version = 1\n");
        std::fs::write(root.join("chamber.toml"), source).unwrap();
        let result = super::imported_chamber_name(&root);
        assert_eq!(result.is_ok(), valid);
        if valid {
            assert_eq!(result.unwrap(), name);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
