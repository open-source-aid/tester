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

## CGM

`tester cgm` drives the `cgm-dexcom` engine against a board impersonating a Dexcom sensor, over the
same radio.

```sh
cargo run -p tester --features radio -- cgm --model g6 --credential 8GH42Z --console /dev/cu.usbmodem1101
cargo run -p tester --features radio -- cgm --model g7 --pairing-secret 1234 --credential <32 hex characters> --console /dev/cu.usbmodem1101
cargo run -p tester --features radio -- cgm --model one-plus --name DX0234 --credential <32 hex characters> --scenario "live readings" --minutes 30
```

`--model` is `g6`, `g7`, `one-plus` or `flex`. The name to scan for is worked out from the printed
secret — a G6 advertises `Dexcom` plus the last two characters of its transmitter ID, a
G7-generation sensor its prefix plus the last two digits of its pairing code — and `--name` gives it
outright. A `--credential` is the key an earlier pairing agreed (hex) except on the G6, whose
credential is the six characters printed on the transmitter; `--pairing-secret` is what a person
reads off the device. `--scenario` picks one of `pair`, `status`, `live readings`,
`backfill after a gap`, `full cycle` and `fresh pairing`, and defaults to all of them.

What the engine stores — the session, which holds the credential, and the activation anchor — is
kept in `--store`, by default `~/.local/state/tester-cgm/<advertised name>`, so a sensor is paired
once and reconnected to afterwards. Those files are not encrypted: the host's `Wrap` is the identity
here, and there is no key store to do better with.

A fresh G7-generation pairing cannot finish against the simulator: its EC-J-PAKE packets are frozen
bytes computed with fixed scalars and a real host draws its own, so the `fresh pairing` scenario is
reported as skipped with that reason. A fresh G6 pairing and every G7 reconnection do work. Unlike
a pump name, a Dexcom name carries no simulator marker — the shape is all that can be checked, so
the board must be the only thing in range answering to the name given.

Over BLE the runner only connects to a peripheral whose advertised name equals `--name` exactly, and refuses any name that does not end with the simulator serial `3735928559` (0xDEADBEEF): real pumps may be in range. The radio code (`crates/ble/src/radio.rs`, `crates/cli/src/hardware.rs`) builds only with `--features radio` and is outside the coverage gate; everything else is covered at 100%.

Licence AGPL-3.0-or-later.
