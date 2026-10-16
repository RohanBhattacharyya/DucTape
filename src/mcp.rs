//! Model Context Protocol server, letting AI agents operate the driver station.
//!
//! Every tool goes through the same [`Controller`] as the window, so an agent and a person see
//! the same state and the window shows everything the agent does. Safety rules:
//!
//! - disabling and e-stopping are always allowed
//! - enabling follows the configured [`EnablePolicy`] (simulator only by default), and requires
//!   communication and running robot code
//! - every MCP enable is timed and DucTape disables the robot when it runs out

use std::time::Duration;

use rmcp::handler::server::router::prompt::PromptRouter;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    Implementation, ListResourcesResult, PaginatedRequestParams, PromptMessage,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, Role, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{
    ErrorData, Json, RoleServer, ServerHandler, prompt, prompt_handler, prompt_router, tool,
    tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::EnablePolicy;
use crate::console::{ConsoleLine, LineKind};
use crate::controller::{Controller, Source};
use crate::joystick::{SLOTS, VIRTUAL_ID};
use crate::protocol::{
    AllianceColor, AllianceStation, JoystickDevice, JoystickSlot, Mode, Snapshot,
    VirtualJoystickState,
};

const MAX_WAIT: Duration = Duration::from_secs(120);
const MAX_CONSOLE_LINES: usize = 1000;
const MAX_HOLD: Duration = Duration::from_secs(10);

const INSTRUCTIONS: &str = "\
DucTape is a driver station for FIRST Robotics Competition robots. These tools control a real \
robot or the WPILib simulator running on this machine.

Start with get_status. The robot must have communication (comms) and running robot code before \
it can be enabled. Enables last until disable is called, unless you pass duration_seconds (or \
the user configured a limit; see get_status: mcp_max_enable_secs and mcp_enable_policy).

Prefer run_mode for testing: it enables in a mode for a fixed time, then disables and returns the \
console output. Use read_console with the returned next_seq to follow output incrementally. \
Drive the robot in teleop with set_virtual_joystick (axes are -1..1, Y axes negative is forward).

If anything looks wrong, call disable. emergency_stop is for real emergencies only: the robot \
stays disabled until its roboRIO reboots. People at the computer can disable with Enter and \
e-stop with Space at any time.";

/// A compact result returned by tools that change state
#[derive(Serialize, JsonSchema)]
pub struct Ack {
    /// What happened
    pub message: String,
    /// e.g. "Teleoperated Enabled", "No Robot Code"
    pub status: String,
    pub mode: Mode,
    pub enabled: bool,
    pub estopped: bool,
    pub comms: bool,
    pub code: bool,
    pub simulator: bool,
    pub voltage: f32,
    /// Milliseconds until DucTape disables the robot, if a timed enable is active
    pub auto_disable_in_ms: Option<u64>,
}

impl Ack {
    fn new(message: impl Into<String>, s: &Snapshot) -> Ack {
        Ack {
            message: message.into(),
            status: s.status.clone(),
            mode: s.mode,
            enabled: s.enabled,
            estopped: s.estopped,
            comms: s.comms,
            code: s.code,
            simulator: s.simulator,
            voltage: s.voltage,
            auto_disable_in_ms: s.auto_disable_in_ms,
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub struct StatusReport {
    #[serde(flatten)]
    pub state: Snapshot,
    pub mcp_enable_policy: EnablePolicy,
    /// If set, the longest an MCP enable lasts. Null means enables are untimed
    pub mcp_max_enable_secs: Option<u64>,
    /// Why enabling over MCP would currently fail, or null if it would succeed
    pub enable_blocked_reason: Option<String>,
    /// Pass to read_console as `since` to get only output produced after this call
    pub console_next_seq: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct TeamNumberParams {
    /// FRC team number. The robot is expected at 10.TE.AM.2
    pub team_number: u32,
}

#[derive(Deserialize, JsonSchema)]
pub struct ConnectionParams {
    /// Connect to the roboRIO's USB interface (172.22.11.2) instead of the team network address
    pub use_usb: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct ModeParams {
    pub mode: Mode,
}

#[derive(Deserialize, JsonSchema)]
pub struct AllianceParams {
    pub color: AllianceColor,
    /// Driver station 1, 2 or 3
    pub station: u8,
}

#[derive(Deserialize, JsonSchema)]
pub struct GameDataParams {
    /// Game Specific Message for robot code, at most 3 characters (e.g. "LRL")
    pub game_data: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct EnableParams {
    /// Switch to this mode first. Defaults to the current mode
    pub mode: Option<Mode>,
    /// Disable the robot automatically after this many seconds. Omit to stay enabled until
    /// disable is called (unless the user configured a limit, see get_status)
    pub duration_seconds: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RunModeParams {
    pub mode: Mode,
    /// How long to run, in seconds. A full FRC match is 15s autonomous then 135s teleop
    pub duration_seconds: f64,
    /// Return at most this many console lines produced during the run (default 200)
    pub max_console_lines: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
pub struct RunReport {
    /// Whether the run lasted the full duration
    pub completed: bool,
    /// Why the run ended early, if it did
    pub stopped_because: Option<String>,
    pub ran_for_seconds: f64,
    /// Console output produced during the run
    pub console: Vec<ConsoleLine>,
    /// More output was produced than returned; use read_console to see the rest
    pub console_truncated: bool,
    pub final_state: Ack,
}

#[derive(Deserialize, JsonSchema)]
pub struct ReadConsoleParams {
    /// Only return lines with a sequence number at least this (from a previous next_seq).
    /// Omit to get the most recent lines
    pub since: Option<u64>,
    /// Maximum lines to return, default 100, at most 1000
    pub limit: Option<usize>,
    /// Only lines containing this text (case-insensitive)
    pub contains: Option<String>,
    /// Only these kinds of line
    pub kinds: Option<Vec<LineKind>>,
}

#[derive(Serialize, JsonSchema)]
pub struct ConsoleReport {
    pub lines: Vec<ConsoleLine>,
    /// Pass as `since` next time to continue where this left off
    pub next_seq: u64,
    /// More matching lines exist after these
    pub truncated: bool,
}

#[derive(Serialize, JsonSchema)]
pub struct JoystickReport {
    /// The six ports robot code reads, 0-5
    pub slots: Vec<JoystickSlot>,
    /// Devices not assigned to a port
    pub unassigned: Vec<JoystickDevice>,
    pub virtual_joystick: VirtualJoystickState,
    /// The port the virtual joystick is on, if any
    pub virtual_joystick_slot: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AssignJoystickParams {
    /// Device id from list_joysticks. The virtual joystick is 00000000-0000-0000-0000-000000000000
    pub device_id: Uuid,
    /// Port 0-5. Defaults to the first free port
    pub slot: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
pub struct UnassignJoystickParams {
    pub device_id: Uuid,
}

#[derive(Deserialize, JsonSchema)]
pub struct AxisInput {
    /// 0 left X, 1 left Y, 2 left trigger, 3 right trigger, 4 right X, 5 right Y
    pub axis: usize,
    /// -1.0 to 1.0 (triggers 0.0 to 1.0). For Y axes, negative is forward/up
    pub value: f32,
}

#[derive(Deserialize, JsonSchema)]
pub struct ButtonInput {
    /// 1 A, 2 B, 3 X, 4 Y, 5 left bumper, 6 right bumper, 7 back, 8 start, 9 left stick, 10 right stick
    pub button: usize,
    pub pressed: bool,
}

#[derive(Deserialize, JsonSchema)]
pub struct VirtualJoystickParams {
    pub axes: Option<Vec<AxisInput>>,
    pub buttons: Option<Vec<ButtonInput>>,
    /// D-pad angle in degrees (0 up, 90 right, 180 down, 270 left, multiples of 45), or -1 for released
    pub pov: Option<i16>,
    /// Reset every input to neutral before applying these. Default false
    pub reset_first: Option<bool>,
    /// Hold these inputs for this many milliseconds (max 10000), then return everything to
    /// neutral. Omit to leave the inputs applied
    pub hold_ms: Option<u64>,
    /// Port to put the virtual joystick on if it isn't on one. Defaults to the first free port
    pub slot: Option<usize>,
}

#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    Connected,
    Disconnected,
    CodeRunning,
    CodeStopped,
    Enabled,
    Disabled,
}

#[derive(Deserialize, JsonSchema)]
pub struct WaitParams {
    pub condition: Condition,
    /// Give up after this many seconds (default 10, max 120)
    pub timeout_seconds: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
pub struct WaitReport {
    pub satisfied: bool,
    pub waited_seconds: f64,
    pub state: Ack,
}

#[derive(Deserialize, JsonSchema)]
pub struct TestPromptParams {
    /// "autonomous", "teleoperated" or "test" (default autonomous)
    pub mode: Option<String>,
    /// How long to run (default 15)
    pub seconds: Option<String>,
}

fn secs(value: Option<f64>, default: Duration, max: Duration) -> Result<Duration, String> {
    match value {
        None => Ok(default.min(max)),
        Some(v) if v.is_finite() && v > 0.0 => Ok(Duration::from_secs_f64(v.min(1e9)).min(max)),
        Some(v) => Err(format!(
            "duration must be a positive number of seconds, got {v}"
        )),
    }
}

#[derive(Clone)]
pub struct DucTapeMcp {
    ctrl: Controller,
    tool_router: ToolRouter<Self>,
    prompt_router: PromptRouter<Self>,
}

impl DucTapeMcp {
    pub fn new(ctrl: Controller) -> DucTapeMcp {
        DucTapeMcp {
            ctrl,
            tool_router: Self::tool_router(),
            prompt_router: Self::prompt_router(),
        }
    }

    fn ack(&self, message: impl Into<String>) -> Json<Ack> {
        Json(Ack::new(message, &self.ctrl.snapshot()))
    }

    fn status_report(&self) -> StatusReport {
        StatusReport {
            state: self.ctrl.snapshot(),
            mcp_enable_policy: self.ctrl.enable_policy(),
            mcp_max_enable_secs: self.ctrl.max_enable().map(|d| d.as_secs()),
            enable_blocked_reason: self.ctrl.can_enable(Source::Mcp).err(),
            console_next_seq: self.ctrl.console_next_seq(),
        }
    }

    fn joystick_report(&self) -> JoystickReport {
        let (slots, unassigned) = self.ctrl.joysticks().slots();
        JoystickReport {
            slots,
            unassigned,
            virtual_joystick: self.ctrl.joysticks().virtual_state(),
            virtual_joystick_slot: self.ctrl.joysticks().virtual_slot(),
        }
    }
}

#[tool_router]
impl DucTapeMcp {
    /// Get the full driver station state: connection, robot code, mode, enable state, battery
    /// voltage, alliance, joysticks, and whether MCP is currently allowed to enable the robot.
    #[tool(annotations(title = "Get status", read_only_hint = true))]
    async fn get_status(&self) -> Json<StatusReport> {
        Json(self.status_report())
    }

    /// Set the team number. DucTape connects to that team's robot at 10.TE.AM.2 and disables
    /// the robot while switching.
    #[tool(annotations(title = "Set team number", idempotent_hint = true))]
    async fn set_team_number(
        &self,
        Parameters(p): Parameters<TeamNumberParams>,
    ) -> Result<Json<Ack>, String> {
        self.ctrl.set_team_number(p.team_number)?;
        Ok(self.ack(format!(
            "Team number is {}, connecting to {}",
            p.team_number,
            self.ctrl.ds().target_ip()
        )))
    }

    /// Choose whether to reach the roboRIO over its USB interface instead of the team network.
    #[tool(annotations(title = "Set USB connection", idempotent_hint = true))]
    async fn set_connection(&self, Parameters(p): Parameters<ConnectionParams>) -> Json<Ack> {
        self.ctrl.set_use_usb(p.use_usb);
        self.ack(format!("Connecting to {}", self.ctrl.ds().target_ip()))
    }

    /// Set the robot mode (autonomous, teleoperated or test). If the robot is enabled it is
    /// disabled first, like the official driver station.
    #[tool(annotations(title = "Set mode", idempotent_hint = true))]
    async fn set_mode(&self, Parameters(p): Parameters<ModeParams>) -> Json<Ack> {
        self.ctrl.set_mode(p.mode, Source::Mcp);
        self.ack(format!("Mode is {}", p.mode.label()))
    }

    /// Set the alliance color and driver station number reported to robot code.
    #[tool(annotations(title = "Set alliance station", idempotent_hint = true))]
    async fn set_alliance_station(
        &self,
        Parameters(p): Parameters<AllianceParams>,
    ) -> Result<Json<Ack>, String> {
        self.ctrl.set_alliance(AllianceStation {
            color: p.color,
            station: p.station,
        })?;
        Ok(self.ack(format!("Alliance station is {:?} {}", p.color, p.station)))
    }

    /// Set the Game Specific Message (game data) sent to robot code.
    #[tool(annotations(title = "Set game data", idempotent_hint = true))]
    async fn set_game_data(
        &self,
        Parameters(p): Parameters<GameDataParams>,
    ) -> Result<Json<Ack>, String> {
        self.ctrl.set_game_data(&p.game_data)?;
        Ok(self.ack(format!("Game data is {:?}", p.game_data)))
    }

    /// Enable the robot so it runs and can move. Requires robot communication and running robot
    /// code. It stays enabled until disable is called, or for duration_seconds if given.
    #[tool(annotations(title = "Enable robot", destructive_hint = true))]
    async fn enable(&self, Parameters(p): Parameters<EnableParams>) -> Result<Json<Ack>, String> {
        let duration = p
            .duration_seconds
            .map(|d| secs(Some(d), Duration::MAX, Duration::MAX))
            .transpose()?;
        self.ctrl.can_enable(Source::Mcp)?;
        if let Some(mode) = p.mode {
            self.ctrl.set_mode(mode, Source::Mcp);
        }
        let duration = self.ctrl.enable(Source::Mcp, duration)?;
        Ok(self.ack(match duration {
            Some(d) => format!(
                "Robot enabled; it will be disabled in {:.1}s",
                d.as_secs_f64()
            ),
            None => "Robot enabled until disable is called".to_string(),
        }))
    }

    /// Disable the robot. Always allowed.
    #[tool(annotations(title = "Disable robot", idempotent_hint = true))]
    async fn disable(&self) -> Json<Ack> {
        self.ctrl.disable(Source::Mcp);
        self.ack("Robot disabled")
    }

    /// EMERGENCY STOP. Immediately disables the robot, and it cannot be enabled again until the
    /// roboRIO reboots. Only for real emergencies; use disable otherwise.
    #[tool(annotations(title = "Emergency stop", destructive_hint = true))]
    async fn emergency_stop(&self) -> Json<Ack> {
        self.ctrl.estop(Source::Mcp);
        self.ack("Robot emergency stopped. Reboot the roboRIO to clear it")
    }

    /// Restart the robot program on the roboRIO (or simulator). Disables the robot. Use
    /// wait_for with code_running afterwards.
    #[tool(annotations(title = "Restart robot code"))]
    async fn restart_robot_code(&self) -> Json<Ack> {
        self.ctrl.restart_code(Source::Mcp);
        self.ack("Robot code restart requested")
    }

    /// Reboot the roboRIO. Communication drops for about a minute. This also clears an e-stop.
    #[tool(annotations(title = "Reboot roboRIO", destructive_hint = true))]
    async fn reboot_roborio(&self) -> Json<Ack> {
        self.ctrl.reboot_roborio(Source::Mcp);
        self.ack("roboRIO reboot requested")
    }

    /// Run the robot in a mode for a fixed time, then disable it. Returns whether it ran the
    /// whole time, why it stopped otherwise (disabled by a person, lost communication, code
    /// crashed), and the console output produced while it ran. This is the best way to test
    /// robot code, especially autonomous routines in the simulator.
    #[tool(annotations(title = "Run mode for a duration", destructive_hint = true))]
    async fn run_mode(
        &self,
        Parameters(p): Parameters<RunModeParams>,
    ) -> Result<Json<RunReport>, String> {
        let max = self.ctrl.max_enable().unwrap_or(Duration::MAX);
        let duration = secs(Some(p.duration_seconds), max, max)?;
        self.ctrl.can_enable(Source::Mcp)?;
        let start_seq = self.ctrl.console_next_seq();
        self.ctrl.set_mode(p.mode, Source::Mcp);
        self.ctrl.enable(Source::Mcp, Some(duration))?;
        let started = std::time::Instant::now();

        // The timed enable ends the run; anything else ending it first means it stopped early
        let (_, state) = self
            .ctrl
            .wait_until(duration + Duration::from_secs(1), |s| {
                !s.enabled || !s.comms || !s.code
            })
            .await;
        let ran_for = started.elapsed();
        self.ctrl.disable(Source::Mcp);
        let completed = ran_for + Duration::from_millis(100) >= duration
            && state.comms
            && state.code
            && !state.estopped;

        let stopped_because = (!completed).then(|| {
            if state.estopped {
                "the robot was emergency stopped".to_string()
            } else if !state.comms {
                "robot communication was lost".to_string()
            } else if !state.code {
                "robot code stopped (it may have crashed; check the console)".to_string()
            } else {
                format!(
                    "the robot was disabled ({})",
                    state.last_enable_change_by.as_deref().unwrap_or("unknown")
                )
            }
        });
        let limit = p.max_console_lines.unwrap_or(200).min(MAX_CONSOLE_LINES);
        let (console, _, console_truncated) = self.ctrl.console_since(start_seq, limit, |_| true);
        Ok(Json(RunReport {
            completed,
            stopped_because,
            ran_for_seconds: (ran_for.as_secs_f64() * 100.0).round() / 100.0,
            console,
            console_truncated,
            final_state: Ack::new("Run finished, robot disabled", &self.ctrl.snapshot()),
        }))
    }

    /// Wait until the robot connects, disconnects, starts or stops running code, or is enabled
    /// or disabled. Useful after restart_robot_code, reboot_roborio or starting a simulation.
    #[tool(annotations(title = "Wait for condition", read_only_hint = true))]
    async fn wait_for(
        &self,
        Parameters(p): Parameters<WaitParams>,
    ) -> Result<Json<WaitReport>, String> {
        let timeout = secs(p.timeout_seconds, Duration::from_secs(10), MAX_WAIT)?;
        let started = std::time::Instant::now();
        let condition = p.condition;
        let (satisfied, state) = self
            .ctrl
            .wait_until(timeout, move |s| match condition {
                Condition::Connected => s.comms,
                Condition::Disconnected => !s.comms,
                Condition::CodeRunning => s.comms && s.code,
                Condition::CodeStopped => !s.code,
                Condition::Enabled => s.enabled,
                Condition::Disabled => !s.enabled,
            })
            .await;
        Ok(Json(WaitReport {
            satisfied,
            waited_seconds: (started.elapsed().as_secs_f64() * 100.0).round() / 100.0,
            state: Ack::new(
                if satisfied {
                    "Condition met"
                } else {
                    "Timed out"
                },
                &state,
            ),
        }))
    }

    /// Read robot console output (stdout, WPILib errors and warnings, and DucTape notices).
    /// Use since with the next_seq from a previous call to read incrementally.
    #[tool(annotations(title = "Read console", read_only_hint = true))]
    async fn read_console(
        &self,
        Parameters(p): Parameters<ReadConsoleParams>,
    ) -> Json<ConsoleReport> {
        let limit = p.limit.unwrap_or(100).clamp(1, MAX_CONSOLE_LINES);
        let needle = p.contains.map(|s| s.to_lowercase());
        let kinds = p.kinds;
        let filter = move |line: &ConsoleLine| {
            needle
                .as_ref()
                .is_none_or(|n| line.text.to_lowercase().contains(n))
                && kinds.as_ref().is_none_or(|k| k.contains(&line.kind))
        };
        let (lines, next_seq, truncated) = match p.since {
            Some(since) => self.ctrl.console_since(since, limit, filter),
            None => {
                // The most recent matching lines
                let (all, next, _) = self.ctrl.console_since(0, usize::MAX, filter);
                let truncated = all.len() > limit;
                let lines = all[all.len().saturating_sub(limit)..].to_vec();
                (lines, next, truncated)
            }
        };
        Json(ConsoleReport {
            lines,
            next_seq,
            truncated,
        })
    }

    /// Clear the console buffer.
    #[tool(annotations(title = "Clear console", idempotent_hint = true))]
    async fn clear_console(&self) -> Json<Ack> {
        self.ctrl.clear_console();
        self.ack("Console cleared")
    }

    /// List the six joystick ports robot code reads, which device is on each, and the state of
    /// the virtual joystick that MCP clients can drive.
    #[tool(annotations(title = "List joysticks", read_only_hint = true))]
    async fn list_joysticks(&self) -> Json<JoystickReport> {
        Json(self.joystick_report())
    }

    /// Put a joystick (physical, or the virtual joystick) on a port. A device already on that
    /// port is swapped out.
    #[tool(annotations(title = "Assign joystick", idempotent_hint = true))]
    async fn assign_joystick(
        &self,
        Parameters(p): Parameters<AssignJoystickParams>,
    ) -> Result<Json<JoystickReport>, String> {
        self.ctrl.bind_joystick(p.device_id, p.slot)?;
        Ok(Json(self.joystick_report()))
    }

    /// Remove a joystick from its port, so robot code sees that port as unplugged.
    #[tool(annotations(title = "Unassign joystick", idempotent_hint = true))]
    async fn unassign_joystick(
        &self,
        Parameters(p): Parameters<UnassignJoystickParams>,
    ) -> Result<Json<JoystickReport>, String> {
        self.ctrl.unbind_joystick(p.device_id)?;
        Ok(Json(self.joystick_report()))
    }

    /// Set inputs on the virtual joystick, an Xbox-style controller robot code reads like a real
    /// one. It is put on a port automatically if needed. Inputs not mentioned keep their values
    /// unless reset_first is set. With hold_ms, inputs return to neutral after that long.
    #[tool(annotations(title = "Set virtual joystick"))]
    async fn set_virtual_joystick(
        &self,
        Parameters(p): Parameters<VirtualJoystickParams>,
    ) -> Result<Json<JoystickReport>, String> {
        for axis in p.axes.iter().flatten() {
            if axis.axis >= 6 || !axis.value.is_finite() {
                return Err(format!(
                    "axis must be 0-5 with a finite value, got axis {}",
                    axis.axis
                ));
            }
        }
        for button in p.buttons.iter().flatten() {
            if !(1..=10).contains(&button.button) {
                return Err(format!("buttons are numbered 1-10, got {}", button.button));
            }
        }
        if let Some(pov) = p.pov
            && (pov != -1 && !(0..360).contains(&pov) || pov % 45 != 0)
        {
            return Err("pov must be -1 or a multiple of 45 from 0 to 315".into());
        }
        if let Some(slot) = p.slot
            && slot >= SLOTS
        {
            return Err(format!("joystick ports are 0-{}", SLOTS - 1));
        }

        if self.ctrl.joysticks().virtual_slot().is_none() || p.slot.is_some() {
            self.ctrl.bind_joystick(VIRTUAL_ID, p.slot)?;
        }
        let js = self.ctrl.joysticks();
        if p.reset_first.unwrap_or(false) {
            js.reset_virtual();
        }
        js.update_virtual(|v| {
            for axis in p.axes.iter().flatten() {
                let range = if axis.axis == 2 || axis.axis == 3 {
                    0.0
                } else {
                    -1.0
                };
                v.axes[axis.axis] = axis.value.clamp(range, 1.0);
            }
            for button in p.buttons.iter().flatten() {
                v.buttons[button.button - 1] = button.pressed;
            }
            if let Some(pov) = p.pov {
                v.pov = pov;
            }
        });
        self.ctrl.publish();

        if let Some(hold) = p.hold_ms {
            tokio::time::sleep(Duration::from_millis(hold).min(MAX_HOLD)).await;
            self.ctrl.joysticks().reset_virtual();
            self.ctrl.publish();
        }
        Ok(Json(self.joystick_report()))
    }
}

#[prompt_router]
impl DucTapeMcp {
    /// Test robot code by running it in a mode and reporting what happened.
    #[prompt(name = "test_robot_code")]
    async fn test_robot_code(
        &self,
        Parameters(p): Parameters<TestPromptParams>,
    ) -> Vec<PromptMessage> {
        let mode = p.mode.unwrap_or_else(|| "autonomous".into());
        let seconds = p.seconds.unwrap_or_else(|| "15".into());
        vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Use the DucTape driver station tools to test the robot code.\n\
                 1. Call get_status. If there is no communication or no robot code, explain what \
                    is missing (for a simulation, start it with `./gradlew simulateJava` or \
                    `simulateNative`) and use wait_for with code_running.\n\
                 2. If enable_blocked_reason is set, stop and tell me why.\n\
                 3. Call run_mode with mode {mode} for {seconds} seconds.\n\
                 4. Summarise what the robot did using the console output: errors, warnings, \
                    exceptions, and whether it stopped early. Suggest fixes for any problems."
            ),
        )]
    }

    /// Work out why the driver station can't talk to the robot.
    #[prompt(name = "diagnose_connection")]
    async fn diagnose_connection(&self) -> Vec<PromptMessage> {
        vec![PromptMessage::new_text(
            Role::User,
            "Use the DucTape tools to diagnose why the driver station can't control the robot. \
             Call get_status and check the team number and target_address, whether USB is in \
             use, whether communication is established, whether robot code is running, and the \
             recent console output (read_console). Explain the likely cause and how to fix it. \
             Do not enable the robot."
                .to_string(),
        )]
    }
}

const STATUS_URI: &str = "ductape://status";
const CONSOLE_URI: &str = "ductape://console";
const JOYSTICKS_URI: &str = "ductape://joysticks";

#[tool_handler(router = self.tool_router)]
#[prompt_handler(router = self.prompt_router)]
impl ServerHandler for DucTapeMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_prompts()
                .build(),
        )
        .with_server_info(
            Implementation::new("ductape", env!("CARGO_PKG_VERSION"))
                .with_title("DucTape Driver Station"),
        )
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(STATUS_URI, "status")
                .with_title("Driver station status")
                .with_description("The full driver station state as JSON")
                .with_mime_type("application/json"),
            Resource::new(CONSOLE_URI, "console")
                .with_title("Robot console")
                .with_description("The last 200 lines of robot console output")
                .with_mime_type("text/plain"),
            Resource::new(JOYSTICKS_URI, "joysticks")
                .with_title("Joysticks")
                .with_description("Joystick ports and the virtual joystick state as JSON")
                .with_mime_type("application/json"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let json = |value: serde_json::Result<String>, uri: &str| {
            value
                .map(|text| ResourceContents::text(text, uri).with_mime_type("application/json"))
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))
        };
        let contents = match request.uri.as_str() {
            STATUS_URI => json(
                serde_json::to_string_pretty(&self.status_report()),
                STATUS_URI,
            )?,
            JOYSTICKS_URI => json(
                serde_json::to_string_pretty(&self.joystick_report()),
                JOYSTICKS_URI,
            )?,
            CONSOLE_URI => {
                let text = self
                    .ctrl
                    .console_recent(200)
                    .iter()
                    .map(|l| match l.kind {
                        LineKind::Stdout => l.text.clone(),
                        kind => format!("[{kind:?}] {}", l.text),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                ResourceContents::text(text, CONSOLE_URI).with_mime_type("text/plain")
            }
            uri => {
                return Err(ErrorData::resource_not_found(
                    format!("unknown resource {uri}"),
                    None,
                ));
            }
        };
        Ok(ReadResourceResult::new(vec![contents]).into())
    }
}

/// Serves MCP on stdin/stdout until the client disconnects
pub async fn serve_stdio(ctrl: Controller) -> anyhow::Result<()> {
    use rmcp::ServiceExt;
    let service = DucTapeMcp::new(ctrl)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use ds::testing::FakeRobot;
    use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams};
    use rmcp::service::RunningService;
    use rmcp::{ClientHandler, RoleClient, ServiceExt};
    use serde_json::{Value, json};

    use super::*;
    use crate::config::Config;
    use crate::controller::Options;

    #[derive(Default, Clone)]
    struct TestClient;
    impl ClientHandler for TestClient {}

    async fn setup(
        robot: &FakeRobot,
        policy: EnablePolicy,
        max_enable: Option<Duration>,
    ) -> (Controller, RunningService<RoleClient, TestClient>) {
        let ctrl = Controller::start(
            Options {
                team_number: 0,
                use_usb: false,
                robot_ip: Some(Ipv4Addr::LOCALHOST.into()),
                alliance: AllianceStation::default(),
                enable_policy: policy,
                max_enable,
                persist: false,
                ports: Some(robot.ports()),
            },
            Config::default(),
        );
        let (server_io, client_io) = tokio::io::duplex(1 << 16);
        let server = DucTapeMcp::new(ctrl.clone());
        tokio::spawn(async move {
            if let Ok(running) = server.serve(server_io).await {
                let _ = running.waiting().await;
            }
        });
        let client = TestClient.serve(client_io).await.expect("client connects");
        (ctrl, client)
    }

    /// Calls a tool, returning (is_error, structured output or text)
    async fn call(
        client: &RunningService<RoleClient, TestClient>,
        name: &'static str,
        args: Value,
    ) -> (bool, Value) {
        let mut params = CallToolRequestParams::new(name);
        if let Value::Object(map) = args {
            params = params.with_arguments(map);
        }
        let result = client.call_tool(params).await.expect("tool call");
        let is_error = result.is_error.unwrap_or(false);
        let value = result.structured_content.clone().unwrap_or_else(|| {
            let text = result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join("\n");
            Value::String(text)
        });
        (is_error, value)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn agent_drives_a_robot_end_to_end() {
        let robot = FakeRobot::start();
        let (_ctrl, client) = setup(&robot, EnablePolicy::default(), None).await;

        let tools: Vec<_> = client
            .list_all_tools()
            .await
            .unwrap()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        for expected in [
            "get_status",
            "enable",
            "disable",
            "emergency_stop",
            "run_mode",
            "set_virtual_joystick",
            "read_console",
            "wait_for",
        ] {
            assert!(
                tools.iter().any(|t| t == expected),
                "missing tool {expected}"
            );
        }

        let (err, wait) = call(
            &client,
            "wait_for",
            json!({"condition": "code_running", "timeout_seconds": 5}),
        )
        .await;
        assert!(!err && wait["satisfied"] == true, "{wait}");

        let (_, status) = call(&client, "get_status", json!({})).await;
        assert_eq!(status["comms"], true);
        assert_eq!(status["enable_blocked_reason"], Value::Null);
        assert_eq!(status["voltage"], 12.5);

        // Drive the virtual joystick; the robot sees it on port 0
        let (err, js) = call(
            &client,
            "set_virtual_joystick",
            json!({"axes": [{"axis": 1, "value": -0.5}], "buttons": [{"button": 1, "pressed": true}]}),
        )
        .await;
        assert!(!err, "{js}");
        assert_eq!(js["virtual_joystick_slot"], 0);
        assert!(robot.wait_for(Duration::from_secs(3), |sim| {
            sim.last_control.as_ref().is_some_and(|p| {
                p.tags.iter().any(|t| matches!(t, ds::proto::udp::UdpTag::Joystick(j) if j.axes.get(1) == Some(&-64) && j.buttons[0]))
            })
        }));

        // A timed run in autonomous, with console output collected along the way
        let printer = {
            let robot_ptr = &robot;
            async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                robot_ptr.print("auto: driving forward");
            }
        };
        let ((err, report), _) = tokio::join!(
            call(
                &client,
                "run_mode",
                json!({"mode": "autonomous", "duration_seconds": 0.8})
            ),
            printer
        );
        assert!(!err, "{report}");
        assert_eq!(report["completed"], true, "{report}");
        assert_eq!(report["final_state"]["enabled"], false);
        let console = report["console"].to_string();
        assert!(
            console.contains("enabled in Autonomous by MCP client"),
            "{console}"
        );
        assert!(console.contains("auto: driving forward"), "{console}");
        assert!(robot.wait_for(Duration::from_secs(1), |sim| {
            sim.last_control.as_ref().is_some_and(|p| !p.enabled())
        }));

        // By default an enable lasts until disable is called
        let (err, ack) = call(&client, "enable", json!({"mode": "teleoperated"})).await;
        assert!(!err && ack["enabled"] == true, "{ack}");
        assert_eq!(ack["auto_disable_in_ms"], Value::Null, "{ack}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (_, status) = call(&client, "get_status", json!({})).await;
        assert_eq!(status["enabled"], true);
        call(&client, "disable", json!({})).await;

        // A timed enable expires on its own
        let (err, ack) = call(
            &client,
            "enable",
            json!({"mode": "teleoperated", "duration_seconds": 0.3}),
        )
        .await;
        assert!(!err && ack["enabled"] == true, "{ack}");
        let (_, wait) = call(
            &client,
            "wait_for",
            json!({"condition": "disabled", "timeout_seconds": 3}),
        )
        .await;
        assert_eq!(wait["satisfied"], true);

        let (_, console) = call(
            &client,
            "read_console",
            json!({"contains": "timed enable", "limit": 5}),
        )
        .await;
        // One from run_mode, one from the timed enable
        assert_eq!(console["lines"].as_array().unwrap().len(), 2, "{console}");

        let status = client
            .read_resource(ReadResourceRequestParams::new(STATUS_URI))
            .await
            .unwrap();
        assert_eq!(status.contents.len(), 1);

        // After an e-stop, enabling is refused with an explanation
        call(&client, "emergency_stop", json!({})).await;
        let (err, msg) = call(&client, "enable", json!({})).await;
        assert!(err);
        assert!(msg.to_string().contains("emergency stopped"), "{msg}");

        client.cancel().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn default_policy_refuses_real_robots() {
        let robot = FakeRobot::start();
        let (ctrl, client) = setup(&robot, EnablePolicy::Simulator, None).await;
        call(&client, "wait_for", json!({"condition": "code_running"})).await;

        let (err, msg) = call(&client, "enable", json!({})).await;
        assert!(err);
        assert!(msg.to_string().contains("simulator"), "{msg}");
        let (_, status) = call(&client, "get_status", json!({})).await;
        assert!(status["enable_blocked_reason"].is_string());

        // People at the window aren't restricted, and MCP can always disable
        ctrl.enable(Source::Ui, None).unwrap();
        let (err, ack) = call(&client, "disable", json!({})).await;
        assert!(!err && ack["enabled"] == false);

        let (err, _) = call(
            &client,
            "set_virtual_joystick",
            json!({"buttons": [{"button": 11, "pressed": true}]}),
        )
        .await;
        assert!(err, "out of range buttons are rejected");
        client.cancel().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn configured_limit_times_enables() {
        let robot = FakeRobot::start();
        let (_ctrl, client) = setup(
            &robot,
            EnablePolicy::Always,
            Some(Duration::from_millis(500)),
        )
        .await;
        call(&client, "wait_for", json!({"condition": "code_running"})).await;

        let (err, ack) = call(&client, "enable", json!({"duration_seconds": 60})).await;
        assert!(!err, "{ack}");
        let remaining = ack["auto_disable_in_ms"].as_u64().expect("enable is timed");
        assert!(remaining <= 500, "{ack}");
        let (_, wait) = call(
            &client,
            "wait_for",
            json!({"condition": "disabled", "timeout_seconds": 3}),
        )
        .await;
        assert_eq!(wait["satisfied"], true);
        client.cancel().await.unwrap();
    }
}
