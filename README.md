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
cargo run -p tester -- run --target sim --mode encrypted
```

`--mode opn` (the default) drives a pump with no CryptoService through a version-4 session blob, matching the simulator's default firmware. `--mode encrypted` drives an encrypted pump (session key `[0xA5; 32]`) through a version-1 blob, matching the simulator built with `--no-default-features`. Both modes run the same scenarios: every driver command, every board console knob, and the fault sweeps of `pump-yspo`'s `tests/e2e.rs` (a timeout, disconnect, host failure, counter error or 0x87 refusal injected at each step of a connect and of every command). Each scenario prints `pass`, `FAIL` or `skip` with the reason: a scenario that does not apply to the mode (counters, epoch and keys on an OPN pump) or that needs pump state the board console cannot reach (`--target ble`) is skipped, never silently. A fresh session hands the engine the last stored counters, as the phone would. The in-process double uses the offline-pairing server key `0xCAFE` repeated, like the firmware.

Over BLE the runner only connects to a peripheral whose advertised name equals `--name` exactly, and refuses any name that does not end with the simulator serial `3735928559` (0xDEADBEEF): real pumps may be in range. The radio code (`crates/ble/src/radio.rs`, `crates/cli/src/hardware.rs`) builds only with `--features radio` and is outside the coverage gate; everything else is covered at 100%.

Licence AGPL-3.0-or-later.
