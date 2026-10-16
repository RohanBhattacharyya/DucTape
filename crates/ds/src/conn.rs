//! The I/O thread: a single-threaded tokio runtime that owns every socket.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::task::JoinHandle as TaskHandle;
use tokio::time::{self, MissedTickBehavior};

use crate::driver_station::{DriverStation, DsMode, Ports, Shared, Telemetry};
use crate::proto::tcp::{self, TcpTag};
use crate::proto::udp::{StatusPacket, UdpTag};

/// How often control packets are sent
const SEND_INTERVAL: Duration = Duration::from_millis(20);
/// How long without a status packet before the robot is considered disconnected
const COMMS_TIMEOUT: Duration = Duration::from_millis(1000);
/// How long without a simulator announcement before leaving simulation mode
const SIM_TIMEOUT: Duration = Duration::from_millis(500);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug)]
pub(crate) enum Command {
    /// The connection target changed
    Retarget,
    SendTcp(TcpTag),
    Shutdown,
}

pub(crate) fn spawn(
    shared: Arc<Shared>,
    commands: mpsc::UnboundedReceiver<Command>,
    ports: Ports,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("ds-io".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to start driver station runtime");
            rt.block_on(run(shared, commands, ports));
        })
        .expect("failed to spawn driver station thread")
}

/// Binds a UDP socket. Failures are stored in `error` and logged once rather than on every retry.
async fn bind_udp(addr: SocketAddr, error: &Mutex<Option<String>>) -> Option<UdpSocket> {
    let result = UdpSocket::bind(addr).await;
    let mut error = error.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(sock) => {
            if error.take().is_some() {
                tracing::info!("bound UDP {addr}");
            }
            Some(sock)
        }
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::AddrInUse {
                format!(
                    "UDP port {} is in use: another driver station (or Conductor) is already running",
                    addr.port()
                )
            } else {
                format!("could not bind UDP {addr}: {e}")
            };
            if error.as_deref() != Some(msg.as_str()) {
                tracing::warn!("{msg}; retrying");
                *error = Some(msg);
            }
            None
        }
    }
}

async fn recv(sock: &Option<UdpSocket>, buf: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
    match sock {
        Some(sock) => sock.recv_from(buf).await,
        None => std::future::pending().await,
    }
}

struct Link {
    tcp: Option<TaskHandle<()>>,
    tcp_tx: Option<mpsc::UnboundedSender<TcpTag>>,
}

impl Link {
    fn drop_tcp(&mut self) {
        if let Some(task) = self.tcp.take() {
            task.abort();
        }
        self.tcp_tx = None;
    }
}

fn reset_link(shared: &Shared, link: &mut Link) {
    shared.control().reset_link();
    *shared.telemetry() = Telemetry::default();
    link.drop_tcp();
}

async fn run(shared: Arc<Shared>, mut commands: mpsc::UnboundedReceiver<Command>, ports: Ports) {
    let any = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let tx = UdpSocket::bind(SocketAddr::new(any, 0))
        .await
        .expect("failed to bind UDP send socket");
    let rx_addr = SocketAddr::new(any, ports.ds_udp);
    let mut rx = bind_udp(rx_addr, &shared.link_error).await;
    let sim_addr = ports
        .sim_detect
        .map(|port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    // Not being able to detect the simulator isn't worth reporting beyond the log
    let sim_error = Mutex::new(None);
    let mut sim = match sim_addr {
        Some(addr) => bind_udp(addr, &sim_error).await,
        None => None,
    };
    let mut last_sim_packet: Option<Instant> = None;

    let mut send_tick = time::interval(SEND_INTERVAL);
    send_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut watchdog = time::interval(Duration::from_millis(100));
    watchdog.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut retry = time::interval(RETRY_INTERVAL);

    let mut link = Link {
        tcp: None,
        tcp_tx: None,
    };
    let mut buf = [0u8; 1500];
    let mut sim_buf = [0u8; 64];

    loop {
        tokio::select! {
            _ = send_tick.tick() => {
                let target = SocketAddr::new(shared.control().target_ip(), ports.robot_udp);
                let packet = shared.next_packet().encode();
                // Errors are expected while the robot is unreachable
                let _ = tx.send_to(&packet, target).await;
            }
            res = recv(&rx, &mut buf) => {
                let Ok((len, from)) = res else { continue };
                if from.ip() != shared.control().target_ip() {
                    continue;
                }
                let Some(status) = StatusPacket::decode(&buf[..len]) else {
                    tracing::debug!("malformed status packet from {from}");
                    continue;
                };
                let newly_connected = !shared.telemetry().connected;
                DriverStation::apply_status(&shared, status.status, status.trace, status.battery);
                if status.need_date {
                    let mut control = shared.control();
                    control.pending.push(UdpTag::DateTime(chrono::Utc::now()));
                    control.pending.push(UdpTag::Timezone("UTC".into()));
                }
                if newly_connected {
                    tracing::info!("connected to robot at {}", from.ip());
                    let (tag_tx, tag_rx) = mpsc::unbounded_channel();
                    let addr = SocketAddr::new(from.ip(), ports.robot_tcp);
                    link.drop_tcp();
                    link.tcp = Some(tokio::spawn(tcp_conn(shared.clone(), addr, tag_rx)));
                    link.tcp_tx = Some(tag_tx);
                }
            }
            res = recv(&sim, &mut sim_buf) => {
                if res.is_ok() {
                    last_sim_packet = Some(Instant::now());
                    if shared.control().ds_mode != DsMode::Simulation {
                        tracing::info!("simulator detected, entering simulation mode");
                        shared.control().ds_mode = DsMode::Simulation;
                        reset_link(&shared, &mut link);
                    }
                }
            }
            _ = watchdog.tick() => {
                let timed_out = {
                    let telemetry = shared.telemetry();
                    telemetry.connected
                        && telemetry.last_packet.is_none_or(|t| t.elapsed() > COMMS_TIMEOUT)
                };
                if timed_out {
                    tracing::warn!("lost communication with robot, disabling");
                    reset_link(&shared, &mut link);
                }
                let sim_gone = last_sim_packet.is_none_or(|t| t.elapsed() > SIM_TIMEOUT);
                if sim_gone && shared.control().ds_mode == DsMode::Simulation {
                    tracing::info!("simulator gone, leaving simulation mode");
                    shared.control().ds_mode = DsMode::Normal;
                    reset_link(&shared, &mut link);
                }
            }
            _ = retry.tick(), if rx.is_none() || (sim.is_none() && sim_addr.is_some()) => {
                if rx.is_none() {
                    rx = bind_udp(rx_addr, &shared.link_error).await;
                }
                if let (None, Some(addr)) = (&sim, sim_addr) {
                    sim = bind_udp(addr, &sim_error).await;
                }
            }
            cmd = commands.recv() => match cmd {
                Some(Command::Retarget) => {
                    tracing::info!("retargeting to {}", shared.control().target_ip());
                    reset_link(&shared, &mut link);
                }
                Some(Command::SendTcp(tag)) => {
                    if let Some(tx) = &link.tcp_tx {
                        let _ = tx.send(tag);
                    }
                }
                Some(Command::Shutdown) | None => {
                    link.drop_tcp();
                    return;
                }
            }
        }
    }
}

/// Connects to the robot's TCP port, forwarding received packets to the consumer and
/// queued tags to the robot. Reconnects until aborted.
async fn tcp_conn(
    shared: Arc<Shared>,
    addr: SocketAddr,
    mut tags: mpsc::UnboundedReceiver<TcpTag>,
) {
    loop {
        let stream = match time::timeout(Duration::from_secs(2), TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => stream,
            _ => {
                time::sleep(RETRY_INTERVAL).await;
                continue;
            }
        };
        tracing::debug!("TCP connected to {addr}");
        let _ = stream.set_nodelay(true);
        let (mut reader, mut writer) = stream.into_split();

        let game_data = shared.control().game_data.clone();
        if let Some(gsm) = game_data {
            let _ = writer.write_all(&TcpTag::GameData(gsm).encode()).await;
        }

        let mut buf = BytesMut::with_capacity(4096);
        loop {
            tokio::select! {
                res = reader.read_buf(&mut buf) => {
                    match res {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            while let Some(packet) = tcp::decode_frame(&mut buf) {
                                let mut consumer = shared.tcp_consumer.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(consumer) = consumer.as_mut() {
                                    consumer(packet);
                                }
                            }
                        }
                    }
                }
                Some(tag) = tags.recv() => {
                    if writer.write_all(&tag.encode()).await.is_err() {
                        break;
                    }
                }
            }
        }
        tracing::debug!("TCP disconnected from {addr}");
        time::sleep(RETRY_INTERVAL).await;
    }
}
