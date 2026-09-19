use assert_cmd::Command;

#[test]
fn version_prints_name_and_version() {
    let mut cmd = Command::cargo_bin("singularrag").unwrap();
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::starts_with("singularrag 0.1.0"));
}
