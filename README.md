# tester

End-to-end testing for device drivers built on `pump-shared`: insulin pumps today, CGMs and other devices later.

| Crate | Role |
|---|---|
| `tester-harness` | runs a driver engine against a `SimDevice` in process: virtual time, host actions, fault injection, invariant checks |
| `tester-ble` | real BLE central (`btleplug`, feature `radio`) as a `SimDevice`, the simulator name guard, ATT status recovery, the board serial console |
| `tester` | the `tester` command: one list of scenarios run in process or over BLE against a hardware simulator from `u-osaid/simulator` |

```sh
cargo run -p tester -- run --target sim
cargo run -p tester --features radio -- run --target ble --name YpsoPump_3735928559 --console /dev/cu.usbmodem1101
```

Over BLE the runner only connects to a peripheral whose advertised name equals `--name` exactly, and refuses any name that does not end with the simulator serial `3735928559` (0xDEADBEEF): real pumps may be in range. The radio code (`crates/ble/src/radio.rs`, `crates/cli/src/hardware.rs`) builds only with `--features radio` and is outside the coverage gate; everything else is covered at 100%.

Licence AGPL-3.0-or-later.
