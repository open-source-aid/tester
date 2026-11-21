use std::process::{Command, Output};

fn tester(args: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tester"))
        .args(args.split_whitespace())
        .output()
        .unwrap()
}

#[test]
fn runs_every_scenario_in_process_in_both_modes() {
    for mode in ["opn", "encrypted"] {
        let output = tester(&format!("run --target sim --mode {mode}"));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(output.status.success(), "{stdout}");
        assert_eq!(stdout.lines().count(), 41);
        assert!(!stdout.contains("FAIL"), "{stdout}");
    }
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

#[test]
fn refuses_a_cgm_line_it_cannot_use_before_touching_the_radio() {
    for (args, expected) in [
        ("cgm", "--model is one of g6, g7, one-plus or flex"),
        ("cgm --model g7", "--name is the name the board advertises"),
        ("cgm --model g7 --name Dexcom12", "is a g6 name, not a g7"),
    ] {
        let output = tester(args);
        assert_eq!(output.status.code(), Some(2), "{args}");
        assert!(output.stdout.is_empty(), "{args}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains(expected), "{args}: {error}");
    }
}

#[test]
fn names_its_scenarios_when_asked_for_one_that_does_not_exist() {
    let output = tester("cgm --model g7 --pairing-secret 1234 --scenario pairing");
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8(output.stderr).unwrap();
    for name in ["all", "pair", "status", "full cycle", "fresh pairing"] {
        assert!(error.contains(name), "{error}");
    }
}

#[cfg(not(feature = "radio"))]
#[test]
fn needs_the_radio_feature_for_a_sensor() {
    let output = tester("cgm --model g7 --pairing-secret 1234");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "built without BLE: rebuild with --features radio\n"
    );
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
