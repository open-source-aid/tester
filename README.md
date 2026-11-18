# tester

End-to-end testing for device drivers built on `pump-shared`: insulin pumps today, CGMs and other devices later.

| Crate | Role |
|---|---|
| `tester-harness` | runs a driver engine against a `SimDevice` in process: virtual time, host actions, fault injection, invariant checks |

Planned: scenario files (device state, commands, faults, expected outcomes) and a `tester` command that runs the same scenario in process or over BLE against a hardware simulator from `u-osaid/simulator`.

Licence AGPL-3.0-or-later.
