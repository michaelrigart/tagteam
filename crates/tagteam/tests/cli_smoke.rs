use assert_cmd::Command;

#[test]
fn prints_its_version() {
    Command::cargo_bin("tagteam")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout("tagteam 0.1.0\n");
}
