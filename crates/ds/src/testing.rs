//! A fake roboRIO for testing driver station code without hardware.
//!
//! It listens on localhost, answers control packets with status packets, records what it was
//! sent, and can push console output over TCP.

use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::{mpsc, oneshot};

use crate::Ports;
use crate::proto::tcp::{Stdout, encode_stdout};
use crate::proto::udp::{ControlPacket, Request, Status, StatusPacket, Trace};

/// What the fake robot reports and what it has received
#[derive(Debug, Clone)]
pub struct RobotSim {
    /// Whether to answer control packets at all
    pub online: bool,
    pub code_running: bool,
    pub voltage: f32,
    /// Latched when an e-stop is received, like a real roboRIO; cleared by [`FakeRobot::reboot`]
    pub estopped: bool,
    /// The most recent control packet
    pub last_control: Option<ControlPacket>,
    /// Every request (restart code / reboot) received
    pub requests: Vec<Request>,
    /// Raw bytes received over TCP (game data frames)
    pub tcp_received: Vec<u8>,
}

/// A fake roboRIO running on a background thread. Stops when dropped.
pub struct FakeRobot {
    sim: Arc<Mutex<RobotSim>>,
    stdout_tx: mpsc::UnboundedSender<String>,
    ports: Ports,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

fn free_udp_port() -> u16 {
    StdUdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|s| s.local_addr())
        .map(|a| a.port())
        .expect("no free UDP port")
}

fn free_tcp_port() -> u16 {
    StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|s| s.local_addr())
        .map(|a| a.port())
        .expect("no free TCP port")
}

impl FakeRobot {
    /// Starts a fake robot on random localhost ports. Connect a driver station to it with
    /// `DriverStation::builder().target_ip(Ipv4Addr::LOCALHOST.into()).ports(robot.ports())`.
    pub fn start() -> FakeRobot {
        let ports = Ports {
            robot_udp: free_udp_port(),
            ds_udp: free_udp_port(),
            robot_tcp: free_tcp_port(),
            sim_detect: None,
        };
        let sim = Arc::new(Mutex::new(RobotSim {
            online: true,
            code_running: true,
            voltage: 12.5,
            estopped: false,
            last_control: None,
            requests: Vec::new(),
            tcp_received: Vec::new(),
        }));
        let (stdout_tx, stdout_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread_sim = sim.clone();
        let thread = thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(run(thread_sim, ports, stdout_rx, shutdown_rx, ready_tx));
        });
        ready_rx.recv().expect("fake robot failed to start");
        FakeRobot {
            sim,
            stdout_tx,
            ports,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        }
    }

    pub fn ports(&self) -> Ports {
        self.ports
    }

    pub fn state(&self) -> MutexGuard<'_, RobotSim> {
        self.sim.lock().unwrap()
    }

    /// Simulates a reboot: goes silent long enough for the driver station to notice, then
    /// comes back with the e-stop latch released.
    pub fn reboot(&self) {
        self.state().online = false;
        thread::sleep(Duration::from_millis(1500));
        let mut sim = self.state();
        sim.estopped = false;
        sim.online = true;
    }

    /// Sends a line of console output to any connected driver station
    pub fn print(&self, line: impl Into<String>) {
        let _ = self.stdout_tx.send(line.into());
    }

    /// Polls `cond` until it is true or `timeout` elapses
    pub fn wait_for(&self, timeout: Duration, mut cond: impl FnMut(&RobotSim) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if cond(&self.state()) {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        false
    }
}

impl Drop for FakeRobot {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn run(
    sim: Arc<Mutex<RobotSim>>,
    ports: Ports,
    mut stdout_rx: mpsc::UnboundedReceiver<String>,
    mut shutdown: oneshot::Receiver<()>,
    ready: std::sync::mpsc::Sender<()>,
) {
    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, ports.robot_udp))
        .await
        .unwrap();
    let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, ports.robot_tcp))
        .await
        .unwrap();
    let _ = ready.send(());

    let mut conn: Option<tokio::net::TcpStream> = None;
    let mut buf = [0u8; 1500];
    let mut tcp_buf = [0u8; 256];
    let mut seq = 0u16;
    let started = Instant::now();

    loop {
        tokio::select! {
            _ = &mut shutdown => return,
            res = udp.recv_from(&mut buf) => {
                let Ok((len, from)) = res else { continue };
                let Some(packet) = ControlPacket::decode(&buf[..len]) else { continue };
                let reply = {
                    let mut sim = sim.lock().unwrap();
                    if !packet.request.is_empty() {
                        sim.requests.push(packet.request);
                    }
                    let enabled = packet.enabled();
                    if packet.estopped() {
                        sim.estopped = true;
                    }
                    sim.last_control = Some(packet);
                    if !sim.online {
                        continue;
                    }
                    let mut status = Status::empty();
                    let mut trace = Trace::IS_ROBORIO;
                    if sim.code_running {
                        status |= Status::CODE_START;
                        trace |= Trace::ROBOT_CODE;
                    }
                    if enabled {
                        status |= Status::ENABLED;
                    } else {
                        trace |= Trace::DISABLED;
                    }
                    if sim.estopped {
                        status |= Status::ESTOP;
                    }
                    seq = seq.wrapping_add(1);
                    StatusPacket {
                        seqnum: seq,
                        status,
                        trace,
                        battery: sim.voltage,
                        need_date: false,
                    }
                };
                let _ = udp
                    .send_to(&reply.encode(), SocketAddr::new(from.ip(), ports.ds_udp))
                    .await;
            }
            res = tcp.accept() => {
                if let Ok((stream, _)) = res {
                    conn = Some(stream);
                }
            }
            line = stdout_rx.recv() => {
                let Some(line) = line else { continue };
                if let Some(stream) = conn.as_mut() {
                    let frame = encode_stdout(&Stdout {
                        timestamp: started.elapsed().as_secs_f32(),
                        seqnum: 0,
                        message: line,
                    });
                    if stream.write_all(&frame).await.is_err() {
                        conn = None;
                    }
                }
            }
            res = async {
                match conn.as_mut() {
                    Some(stream) => stream.read(&mut tcp_buf).await,
                    None => std::future::pending().await,
                }
            } => {
                match res {
                    Ok(0) | Err(_) => conn = None,
                    Ok(n) => sim.lock().unwrap().tcp_received.extend_from_slice(&tcp_buf[..n]),
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }
    }
}
