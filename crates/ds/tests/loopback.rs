use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ds::testing::FakeRobot;
use ds::{DriverStation, JoystickValue, Mode, TcpPacket};

const TIMEOUT: Duration = Duration::from_secs(3);

fn connect(robot: &FakeRobot) -> DriverStation {
    DriverStation::builder()
        .target_ip(Ipv4Addr::LOCALHOST.into())
        .ports(robot.ports())
        .connect()
}

fn wait(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + TIMEOUT;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn connects_enables_and_reports_telemetry() {
    let robot = FakeRobot::start();
    let ds = connect(&robot);

    assert!(wait(|| ds.is_connected() && ds.is_code_running()));
    assert!((ds.battery_voltage() - 12.5).abs() < 0.01);

    ds.set_mode(Mode::Teleoperated);
    ds.enable();
    assert!(robot.wait_for(TIMEOUT, |sim| {
        sim.last_control
            .as_ref()
            .is_some_and(|p| p.enabled() && p.mode() == Some(Mode::Teleoperated))
    }));

    ds.restart_code();
    assert!(robot.wait_for(TIMEOUT, |sim| !sim.requests.is_empty()));
}

#[test]
fn disables_when_communication_is_lost() {
    let robot = FakeRobot::start();
    let ds = connect(&robot);
    assert!(wait(|| ds.is_connected()));
    ds.enable();
    assert!(ds.enabled());

    robot.state().online = false;
    assert!(wait(|| !ds.is_connected()));
    assert!(
        !ds.enabled(),
        "robot must not stay enabled after comms loss"
    );

    robot.state().online = true;
    assert!(wait(|| ds.is_connected()));
    assert!(!ds.enabled());
}

#[test]
fn estop_latches_until_robot_reboots() {
    let robot = FakeRobot::start();
    let ds = connect(&robot);
    assert!(wait(|| ds.is_connected()));

    ds.enable();
    ds.estop();
    ds.enable();
    assert!(!ds.enabled());
    assert!(robot.wait_for(TIMEOUT, |sim| sim.estopped));

    // The robot keeps reporting the e-stop, so clearing it locally doesn't stick
    ds.clear_estop();
    std::thread::sleep(Duration::from_millis(200));
    assert!(ds.estopped());

    robot.reboot();
    assert!(wait(|| ds.is_connected()));
    std::thread::sleep(Duration::from_millis(200));
    assert!(!ds.estopped());
    ds.enable();
    assert!(robot.wait_for(TIMEOUT, |sim| {
        sim.last_control
            .as_ref()
            .is_some_and(|p| p.enabled() && !p.estopped())
    }));
}

#[test]
fn joysticks_are_encoded() {
    let robot = FakeRobot::start();
    let ds = connect(&robot);
    ds.set_joystick_supplier(|| {
        vec![
            vec![],
            vec![
                JoystickValue::Axis { id: 1, value: 1.0 },
                JoystickValue::Button {
                    id: 2,
                    pressed: true,
                },
            ],
        ]
    });

    assert!(robot.wait_for(TIMEOUT, |sim| {
        let Some(packet) = &sim.last_control else {
            return false;
        };
        let js: Vec<_> = packet
            .tags
            .iter()
            .filter_map(|t| match t {
                ds::proto::udp::UdpTag::Joystick(js) => Some(js.clone()),
                _ => None,
            })
            .collect();
        js.len() == 2 && js[0].axes.is_empty() && js[1].axes[1] == 127 && js[1].buttons[1]
    }));
}

#[test]
fn console_and_game_data_over_tcp() {
    let robot = FakeRobot::start();
    let ds = connect(&robot);
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = lines.clone();
    ds.set_tcp_consumer(move |packet| {
        if let TcpPacket::Stdout(out) = packet {
            sink.lock().unwrap().push(out.message);
        }
    });
    ds.set_game_specific_message("RBL").unwrap();
    assert!(ds.set_game_specific_message("TOOLONG").is_err());

    assert!(wait(|| ds.is_connected()));
    // Wait for the TCP link, then print
    assert!(robot.wait_for(TIMEOUT, |sim| sim.tcp_received.ends_with(b"RBL")));
    robot.print("Robot program startup complete");
    assert!(wait(|| {
        lines
            .lock()
            .unwrap()
            .iter()
            .any(|l| l == "Robot program startup complete")
    }));
}
