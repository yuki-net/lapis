use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};

#[test]
fn failed_restart_does_not_replace_running_hosts_certificate() {
    let directory = tempfile::tempdir().unwrap();
    let certificate = directory.path().join("host.der");
    let mut first = Command::new(env!("CARGO_BIN_EXE_lapis-host"))
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(&certificate)
        .arg("--listen")
        .arg("127.0.0.1:0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(first.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let address = line
        .split_whitespace()
        .find_map(|part| part.strip_prefix("address="))
        .expect("Hostの待受アドレス");
    let original = std::fs::read(&certificate).unwrap();
    let second = Command::new(env!("CARGO_BIN_EXE_lapis-host"))
        .arg("--workspace")
        .arg(directory.path())
        .arg("--certificate")
        .arg(&certificate)
        .arg("--replace-certificate")
        .arg("--listen")
        .arg(address)
        .output()
        .unwrap();
    writeln!(first.stdin.as_mut().unwrap(), "quit").unwrap();
    assert!(first.wait().unwrap().success());
    assert!(!second.status.success());
    assert_eq!(std::fs::read(certificate).unwrap(), original);
}
