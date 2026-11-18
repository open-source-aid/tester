use std::process::{Command, Output};

fn tester(args: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tester"))
        .args(args.split_whitespace())
        .output()
        .unwrap()
}

#[test]
fn runs_every_scenario_in_process() {
    let output = tester("run --target sim");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");
    assert_eq!(stdout.lines().filter(|l| l.starts_with("pass")).count(), 8);
}

#[test]
fn refuses_a_name_that_is_not_a_simulator() {
    let output = tester("run --target ble --name YpsoPump_10155555");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .starts_with("refusing `YpsoPump_10155555`"));
}

#[cfg(not(feature = "radio"))]
#[test]
fn needs_the_radio_feature_for_ble() {
    let output = tester("run --target ble --name YpsoPump_3735928559");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "built without BLE: rebuild with --features radio\n"
    );
}
