//! The single owner of driver station state.
//!
//! The web UI, global hotkeys and MCP clients all act through a [`Controller`], which applies
//! the safety rules (who may enable, timed enables, disabling on controller loss) and broadcasts
//! a fresh [`Snapshot`] to every subscriber whenever anything changes.

use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;
use uuid::Uuid;

use crate::config::{self, Config, EnablePolicy};
use crate::console::{Console, ConsoleLine, LineKind};
use crate::joystick::{self, InputEvent, Joysticks};
use crate::protocol::{AllianceStation, ClientCommand, Info, Mode, Snapshot};

const POLL_INTERVAL: Duration = Duration::from_millis(50);
pub const MAX_TEAM_NUMBER: u32 = 25599;

/// Who asked for a change. Only MCP clients are subject to the enable policy.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Ui,
    Hotkey,
    Mcp,
    /// Conductor itself, e.g. after a controller was unplugged
    Safety,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Ui => "ui",
            Source::Hotkey => "hotkey",
            Source::Mcp => "mcp",
            Source::Safety => "safety",
        }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    State(Arc<Snapshot>),
    Console(ConsoleLine),
    ConsoleCleared,
}

/// Startup options, usually from the command line and saved config
#[derive(Clone, Debug)]
pub struct Options {
    pub team_number: u32,
    pub use_usb: bool,
    pub robot_ip: Option<IpAddr>,
    pub alliance: AllianceStation,
    pub enable_policy: EnablePolicy,
    pub max_enable: Duration,
    /// Save team number / USB / alliance changes to the config file
    pub persist: bool,
    /// Only tests change these
    pub ports: Option<ds::Ports>,
}

impl Options {
    pub fn from_config(cfg: &Config) -> Options {
        Options {
            team_number: cfg.team_number,
            use_usb: cfg.use_usb,
            robot_ip: None,
            alliance: cfg.alliance,
            enable_policy: cfg.mcp.enable_policy,
            max_enable: Duration::from_secs(cfg.mcp.max_enable_secs.max(1)),
            persist: true,
            ports: None,
        }
    }
}

/// Console lines and the event channel, shared with driver station callbacks
struct Hub {
    console: Mutex<Console>,
    events: broadcast::Sender<Event>,
}

impl Hub {
    fn push(&self, kind: LineKind, text: String) {
        let line = self
            .console
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(kind, text);
        let _ = self.events.send(Event::Console(line));
    }
}

struct Local {
    backend_keybinds: bool,
    auto_disable_at: Option<Instant>,
    last_enable_change_by: Option<Source>,
    last_published: Option<Arc<Snapshot>>,
    prev_comms: bool,
    prev_code: bool,
    mcp_url: Option<String>,
}

struct Inner {
    ds: ds::DriverStation,
    joysticks: Arc<Joysticks>,
    hub: Arc<Hub>,
    local: Mutex<Local>,
    opts: Options,
    config: Mutex<Config>,
}

#[derive(Clone)]
pub struct Controller(Arc<Inner>);

impl Controller {
    /// Connects to the robot and starts background work. Must be called within a tokio runtime.
    pub fn start(opts: Options, config: Config) -> Controller {
        let (events, _) = broadcast::channel(1024);
        let hub = Arc::new(Hub {
            console: Mutex::new(Console::default()),
            events,
        });

        let mut builder = ds::DriverStation::builder()
            .team_number(opts.team_number)
            .alliance(opts.alliance.to_ds());
        if let Some(ip) = opts.robot_ip {
            builder = builder.target_ip(ip);
        }
        if let Some(ports) = opts.ports {
            builder = builder.ports(ports);
        }
        let ds = builder.connect();
        ds.set_use_usb(opts.use_usb);

        let joysticks = Arc::new(Joysticks::default());
        let supplier = joysticks.clone();
        ds.set_joystick_supplier(move || supplier.supply());

        let console_hub = hub.clone();
        ds.set_tcp_consumer(move |packet| match packet {
            ds::TcpPacket::Stdout(out) => console_hub.push(LineKind::Stdout, out.message),
            ds::TcpPacket::Error(err) => {
                let kind = if err.is_error {
                    LineKind::Error
                } else {
                    LineKind::Warning
                };
                let mut text = format!(
                    "{} {}",
                    if err.is_error { "ERROR" } else { "Warning" },
                    err.code
                );
                for part in [&err.details, &err.location, &err.call_stack] {
                    if !part.is_empty() {
                        text.push_str("  ");
                        text.push_str(part);
                    }
                }
                console_hub.push(kind, text);
            }
            ds::TcpPacket::Other { .. } => {}
        });

        let controller = Controller(Arc::new(Inner {
            ds,
            joysticks,
            hub,
            local: Mutex::new(Local {
                backend_keybinds: false,
                auto_disable_at: None,
                last_enable_change_by: None,
                last_published: None,
                prev_comms: false,
                prev_code: false,
                mcp_url: None,
            }),
            opts,
            config: Mutex::new(config),
        }));

        let weak = Arc::downgrade(&controller.0);
        joystick::spawn_input_thread(controller.0.joysticks.clone(), move |event| {
            if let Some(inner) = weak.upgrade() {
                Controller(inner).on_input(event);
            }
        });

        let weak = Arc::downgrade(&controller.0);
        tokio::spawn(poll(weak));

        controller.notice(format!(
            "Conductor {} started, connecting to {}",
            env!("CARGO_PKG_VERSION"),
            controller.0.ds.target_ip()
        ));
        controller
    }

    fn local(&self) -> MutexGuard<'_, Local> {
        self.0.local.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn ds(&self) -> &ds::DriverStation {
        &self.0.ds
    }

    pub fn joysticks(&self) -> &Joysticks {
        &self.0.joysticks
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.hub.events.subscribe()
    }

    pub fn enable_policy(&self) -> EnablePolicy {
        self.0.opts.enable_policy
    }

    pub fn max_enable(&self) -> Duration {
        self.0.opts.max_enable
    }

    pub fn info(&self) -> Info {
        Info {
            version: env!("CARGO_PKG_VERSION").to_string(),
            mcp_url: self.local().mcp_url.clone(),
            mcp_enable_policy: self.0.opts.enable_policy,
            mcp_max_enable_secs: self.0.opts.max_enable.as_secs(),
        }
    }

    pub fn set_mcp_url(&self, url: Option<String>) {
        self.local().mcp_url = url;
    }

    pub fn set_backend_keybinds(&self, enabled: bool) {
        self.local().backend_keybinds = enabled;
        self.publish();
    }

    pub fn snapshot(&self) -> Snapshot {
        let ds = &self.0.ds;
        let telemetry = ds.telemetry();
        let (joysticks, unbound_joysticks) = self.0.joysticks.slots();
        let local = self.local();
        let mode = Mode::from_ds(ds.mode());
        let enabled = ds.enabled();
        let estopped = ds.estopped();
        let status = if estopped {
            "Emergency Stopped".to_string()
        } else if !telemetry.connected {
            "No Robot Communication".to_string()
        } else if !telemetry.code_running {
            "No Robot Code".to_string()
        } else {
            format!(
                "{} {}",
                mode.label(),
                if enabled { "Enabled" } else { "Disabled" }
            )
        };
        Snapshot {
            team_number: ds.team_number(),
            use_usb: ds.use_usb(),
            target_address: ds.target_ip().to_string(),
            mode,
            alliance: AllianceStation::from_ds(ds.alliance()),
            game_data: ds.game_specific_message().unwrap_or_default(),
            enabled,
            estopped,
            comms: telemetry.connected,
            code: telemetry.code_running,
            simulator: ds.ds_mode() == ds::DsMode::Simulation,
            brownout: telemetry.brownout,
            voltage: (telemetry.voltage * 100.0).round() / 100.0,
            status,
            link_error: ds.link_error(),
            joysticks,
            unbound_joysticks,
            virtual_joystick: self.0.joysticks.virtual_state(),
            physical_joysticks: self.0.joysticks.has_physical(),
            backend_keybinds: local.backend_keybinds,
            auto_disable_in_ms: local
                .auto_disable_at
                .filter(|_| enabled)
                .map(|at| at.saturating_duration_since(Instant::now()).as_millis() as u64),
            last_enable_change_by: local.last_enable_change_by.map(|s| s.as_str().to_string()),
        }
    }

    /// Broadcasts the current state if it changed since the last broadcast
    pub fn publish(&self) {
        let snapshot = Arc::new(self.snapshot());
        let mut local = self.local();
        if local.last_published.as_deref() != Some(&*snapshot) {
            local.last_published = Some(snapshot.clone());
            drop(local);
            let _ = self.0.hub.events.send(Event::State(snapshot));
        }
    }

    /// Adds a Conductor notice to the robot console
    pub fn notice(&self, text: impl Into<String>) {
        let text = text.into();
        tracing::info!("{text}");
        self.0.hub.push(LineKind::Conductor, text);
    }

    fn update_config(&self, f: impl FnOnce(&mut Config)) {
        if !self.0.opts.persist {
            return;
        }
        let mut cfg = self.0.config.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut cfg);
        config::store(&cfg);
    }

    pub fn set_team_number(&self, team: u32) -> Result<(), String> {
        if team > MAX_TEAM_NUMBER {
            return Err(format!("team numbers go up to {MAX_TEAM_NUMBER}"));
        }
        if team != self.0.ds.team_number() {
            self.clear_auto_disable();
            self.0.ds.set_team_number(team);
            self.update_config(|c| c.team_number = team);
            self.notice(format!(
                "Team number set to {team}, connecting to {}",
                self.0.ds.target_ip()
            ));
        }
        self.publish();
        Ok(())
    }

    pub fn set_use_usb(&self, use_usb: bool) {
        if use_usb != self.0.ds.use_usb() {
            self.clear_auto_disable();
            self.0.ds.set_use_usb(use_usb);
            self.update_config(|c| c.use_usb = use_usb);
            self.notice(format!("Connecting to {}", self.0.ds.target_ip()));
        }
        self.publish();
    }

    pub fn set_game_data(&self, game_data: &str) -> Result<(), String> {
        self.0
            .ds
            .set_game_specific_message(game_data)
            .map_err(|e| e.to_string())?;
        self.publish();
        Ok(())
    }

    /// Changes mode. Like the NI driver station, this disables the robot if it was enabled.
    pub fn set_mode(&self, mode: Mode, source: Source) {
        if Mode::from_ds(self.0.ds.mode()) != mode {
            if self.0.ds.enabled() {
                self.disable_inner(source);
                self.notice("Robot disabled because the mode changed");
            }
            self.0.ds.set_mode(mode.to_ds());
            if source == Source::Mcp {
                self.notice(format!("Mode set to {} by MCP client", mode.label()));
            }
        }
        self.publish();
    }

    pub fn set_alliance(&self, alliance: AllianceStation) -> Result<(), String> {
        if !(1..=3).contains(&alliance.station) {
            return Err("alliance station must be 1, 2 or 3".into());
        }
        self.0.ds.set_alliance(alliance.to_ds());
        self.update_config(|c| c.alliance = alliance);
        self.publish();
        Ok(())
    }

    /// Checks whether `source` may enable the robot right now
    pub fn can_enable(&self, source: Source) -> Result<(), String> {
        let ds = &self.0.ds;
        if ds.estopped() {
            return Err(
                "The robot is emergency stopped. Reboot the roboRIO to clear the e-stop.".into(),
            );
        }
        if source == Source::Mcp {
            match self.0.opts.enable_policy {
                EnablePolicy::Never => {
                    return Err("Enabling over MCP is turned off (--mcp-enable never). \
                         A person must enable the robot from the Conductor window."
                        .into());
                }
                EnablePolicy::Simulator if ds.ds_mode() != ds::DsMode::Simulation => {
                    return Err(
                        "MCP clients may only enable the WPILib simulator, and Conductor \
                         is connected to a real robot (or nothing). Start a simulation, or ask \
                         the user to relaunch Conductor with --mcp-enable always."
                            .into(),
                    );
                }
                _ => {}
            }
        }
        let telemetry = ds.telemetry();
        if let Some(error) = ds.link_error() {
            return Err(format!(
                "{error}. Close the other driver station, or attach to the running Conductor \
                 window with `conductor --mcp-http` and an HTTP MCP connection."
            ));
        }
        if !telemetry.connected {
            return Err(format!(
                "No robot communication (trying {}). Check the team number and connection.",
                ds.target_ip()
            ));
        }
        if !telemetry.code_running {
            return Err("No robot code is running. Deploy code or restart it first.".into());
        }
        Ok(())
    }

    /// Enables the robot. MCP enables always time out, after at most the configured maximum.
    /// Returns how long the robot will stay enabled, if limited.
    pub fn enable(
        &self,
        source: Source,
        duration: Option<Duration>,
    ) -> Result<Option<Duration>, String> {
        self.can_enable(source)?;
        let duration = match source {
            Source::Mcp => Some(
                duration
                    .unwrap_or(self.0.opts.max_enable)
                    .min(self.0.opts.max_enable),
            ),
            _ => duration,
        };
        self.0.ds.enable();
        {
            let mut local = self.local();
            local.auto_disable_at = duration.map(|d| Instant::now() + d);
            local.last_enable_change_by = Some(source);
        }
        if source == Source::Mcp {
            self.notice(format!(
                "Robot enabled in {} by MCP client{}",
                Mode::from_ds(self.0.ds.mode()).label(),
                duration
                    .map(|d| format!(" for {:.1}s", d.as_secs_f32()))
                    .unwrap_or_default()
            ));
        }
        self.publish();
        Ok(duration)
    }

    fn disable_inner(&self, source: Source) {
        let was_enabled = self.0.ds.enabled();
        self.0.ds.disable();
        let mut local = self.local();
        local.auto_disable_at = None;
        if was_enabled {
            local.last_enable_change_by = Some(source);
        }
    }

    fn clear_auto_disable(&self) {
        self.local().auto_disable_at = None;
    }

    pub fn disable(&self, source: Source) {
        let was_enabled = self.0.ds.enabled();
        self.disable_inner(source);
        if was_enabled && source == Source::Mcp {
            self.notice("Robot disabled by MCP client");
        }
        self.publish();
    }

    pub fn estop(&self, source: Source) {
        self.0.ds.estop();
        {
            let mut local = self.local();
            local.auto_disable_at = None;
            local.last_enable_change_by = Some(source);
        }
        self.notice(format!(
            "EMERGENCY STOP ({})",
            match source {
                Source::Ui => "from the window",
                Source::Hotkey => "space bar",
                Source::Mcp => "by MCP client",
                Source::Safety => "by Conductor",
            }
        ));
        self.publish();
    }

    pub fn restart_code(&self, source: Source) {
        self.disable_inner(source);
        self.0.ds.restart_code();
        self.notice("Restarting robot code");
        self.publish();
    }

    pub fn reboot_roborio(&self, source: Source) {
        self.disable_inner(source);
        self.0.ds.restart_roborio();
        self.notice("Rebooting roboRIO");
        self.publish();
    }

    pub fn move_joystick(&self, from: usize, to: usize) -> Result<(), String> {
        self.0.joysticks.move_slot(from, to)?;
        self.publish();
        Ok(())
    }

    pub fn bind_joystick(&self, id: Uuid, slot: Option<usize>) -> Result<usize, String> {
        let slot = self.0.joysticks.bind(id, slot)?;
        self.publish();
        Ok(slot)
    }

    pub fn unbind_joystick(&self, id: Uuid) -> Result<(), String> {
        self.0.joysticks.unbind(id)?;
        self.publish();
        Ok(())
    }

    pub fn console_recent(&self, n: usize) -> Vec<ConsoleLine> {
        self.0
            .hub
            .console
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .recent(n)
    }

    /// Returns `(lines, next_seq, truncated)`
    pub fn console_since(
        &self,
        since: u64,
        limit: usize,
        filter: impl Fn(&ConsoleLine) -> bool,
    ) -> (Vec<ConsoleLine>, u64, bool) {
        let console = self.0.hub.console.lock().unwrap_or_else(|e| e.into_inner());
        let (lines, truncated) = console.since(since, limit, filter);
        let next = match (truncated, lines.last()) {
            (true, Some(last)) => last.seq + 1,
            _ => console.next_seq(),
        };
        (lines, next, truncated)
    }

    pub fn console_next_seq(&self) -> u64 {
        self.0
            .hub
            .console
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_seq()
    }

    pub fn clear_console(&self) {
        self.0
            .hub
            .console
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        let _ = self.0.hub.events.send(Event::ConsoleCleared);
    }

    /// Waits until `pred` holds for the current state, or `timeout` passes.
    pub async fn wait_until(
        &self,
        timeout: Duration,
        pred: impl Fn(&Snapshot) -> bool,
    ) -> (bool, Snapshot) {
        let mut events = self.subscribe();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let snapshot = self.snapshot();
            if pred(&snapshot) {
                return (true, snapshot);
            }
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Err(_) => return (false, self.snapshot()),
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    tokio::time::sleep(POLL_INTERVAL).await;
                }
                Ok(_) => {}
            }
        }
    }

    /// Applies a command from the web UI
    pub fn apply(&self, command: ClientCommand, source: Source) -> Result<(), String> {
        match command {
            ClientCommand::SetTeamNumber { team_number } => self.set_team_number(team_number)?,
            ClientCommand::SetUseUsb { use_usb } => self.set_use_usb(use_usb),
            ClientCommand::SetGameData { game_data } => self.set_game_data(&game_data)?,
            ClientCommand::SetMode { mode } => self.set_mode(mode, source),
            ClientCommand::SetAlliance { alliance } => self.set_alliance(alliance)?,
            ClientCommand::Enable => {
                self.enable(source, None)?;
            }
            ClientCommand::Disable => self.disable(source),
            ClientCommand::Estop => self.estop(source),
            ClientCommand::RestartCode => self.restart_code(source),
            ClientCommand::RebootRoborio => self.reboot_roborio(source),
            ClientCommand::MoveJoystick { from, to } => self.move_joystick(from, to)?,
            ClientCommand::BindJoystick { id, slot } => {
                self.bind_joystick(id, slot)?;
            }
            ClientCommand::UnbindJoystick { id } => self.unbind_joystick(id)?,
            ClientCommand::ClearConsole => self.clear_console(),
        }
        Ok(())
    }

    fn on_input(&self, event: InputEvent) {
        match event {
            InputEvent::Connected(name) => self.notice(format!("Controller connected: {name}")),
            InputEvent::Removed(removed) => {
                if removed.was_bound && self.0.ds.enabled() {
                    self.disable_inner(Source::Safety);
                    self.notice(format!(
                        "Robot disabled because {} was unplugged",
                        removed.name
                    ));
                } else {
                    self.notice(format!("Controller disconnected: {}", removed.name));
                }
            }
        }
        self.publish();
    }

    /// Runs every poll interval: timed enables, connection notices, state broadcast
    fn tick(&self) {
        let telemetry = self.0.ds.telemetry();
        let enabled = self.0.ds.enabled();

        let mut notices = Vec::new();
        let mut timed_out = false;
        {
            let mut local = self.local();
            if telemetry.connected != local.prev_comms {
                local.prev_comms = telemetry.connected;
                if telemetry.connected {
                    notices.push(format!(
                        "Robot communication established ({})",
                        self.0.ds.target_ip()
                    ));
                } else {
                    notices.push("Robot communication lost".to_string());
                    if local.auto_disable_at.take().is_some() {
                        local.last_enable_change_by = Some(Source::Safety);
                    }
                }
            }
            if telemetry.code_running != local.prev_code {
                local.prev_code = telemetry.code_running;
                if telemetry.connected {
                    notices.push(if telemetry.code_running {
                        "Robot code started".to_string()
                    } else {
                        "Robot code stopped".to_string()
                    });
                }
            }
            if !enabled {
                local.auto_disable_at = None;
            } else if local.auto_disable_at.is_some_and(|at| Instant::now() >= at) {
                timed_out = true;
            }
        }
        for notice in notices {
            self.notice(notice);
        }
        if timed_out {
            self.disable_inner(Source::Mcp);
            self.notice("Timed enable finished, robot disabled");
        }
        self.publish();
    }

    /// Disables the robot and gives the driver station time to tell the robot before exiting
    pub fn shutdown(&self) {
        self.0.ds.disable();
        std::thread::sleep(Duration::from_millis(100));
    }
}

async fn poll(weak: Weak<Inner>) {
    let mut interval = tokio::time::interval(POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let Some(inner) = weak.upgrade() else { return };
        Controller(inner).tick();
    }
}
