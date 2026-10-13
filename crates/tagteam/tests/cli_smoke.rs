use assert_cmd::Command;

#[test]
fn prints_its_version() {
    Command::cargo_bin("tagteam")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(concat!("tagteam ", env!("CARGO_PKG_VERSION"), "\n"));
}
