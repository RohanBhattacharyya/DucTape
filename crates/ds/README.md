# ds

A library for controlling FIRST Robotics Competition robots over the driver
station protocol (2020+ roboRIO / WPILib simulator).

This is the `ds` crate originally published by Kaitlyn Kenwell
([gitlab.com/Redrield/ds-rs](https://gitlab.com/Redrield/ds-rs)), vendored into
DucTape (a revival of Conductor, which it was written for) and ported to tokio 1. Compared to `ds` 1.x it:

- runs on a single dedicated I/O thread with no `block_on` in the public API
- disables the robot when communications are lost (it previously stayed enabled)
- lets callers clear an e-stop after the roboRIO reboots (`clear_estop`)
- re-sends game data whenever the TCP link reconnects
- decodes WPILib error/warning messages (tag `0x0b`) as well as stdout
- has configurable ports so it can be tested against a fake roboRIO

The wire format follows <https://frcture.readthedocs.io>.

```rust,no_run
use ds::{Alliance, DriverStation, Mode};

let ds = DriverStation::new_team(4069, Alliance::new_red(1));
ds.set_mode(Mode::Teleoperated);
if ds.is_code_running() {
    ds.enable();
}
```

Licensed under either of MIT or Apache-2.0 at your option.
