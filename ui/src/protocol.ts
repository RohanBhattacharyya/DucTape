// Mirrors src/protocol.rs and src/console.rs. The backend owns all state; the UI renders the
// latest Snapshot and sends commands.

export type Mode = "autonomous" | "teleoperated" | "test";
export const MODES: Mode[] = ["autonomous", "teleoperated", "test"];
export const MODE_LABELS: Record<Mode, string> = {
  autonomous: "Autonomous",
  teleoperated: "Teleoperated",
  test: "Test",
};

export type AllianceColor = "red" | "blue";

export interface AllianceStation {
  color: AllianceColor;
  station: number;
}

export interface JoystickDevice {
  id: string;
  name: string;
  virtual: boolean;
}

export interface JoystickSlot {
  slot: number;
  device: JoystickDevice | null;
}

export interface VirtualJoystickState {
  axes: number[];
  buttons: boolean[];
  pov: number;
}

export interface Snapshot {
  team_number: number;
  use_usb: boolean;
  target_address: string;
  mode: Mode;
  alliance: AllianceStation;
  game_data: string;
  enabled: boolean;
  estopped: boolean;
  comms: boolean;
  code: boolean;
  simulator: boolean;
  brownout: boolean;
  voltage: number;
  status: string;
  link_error: string | null;
  joysticks: JoystickSlot[];
  unbound_joysticks: JoystickDevice[];
  virtual_joystick: VirtualJoystickState;
  physical_joysticks: boolean;
  backend_keybinds: boolean;
  auto_disable_in_ms: number | null;
  last_enable_change_by: "ui" | "hotkey" | "mcp" | "safety" | null;
}

export type EnablePolicy = "never" | "simulator" | "always";

export interface Info {
  version: string;
  mcp_url: string | null;
  mcp_enable_policy: EnablePolicy;
  mcp_max_enable_secs: number | null;
}

export type LineKind = "stdout" | "error" | "warning" | "ductape";

export interface ConsoleLine {
  seq: number;
  time_ms: number;
  kind: LineKind;
  text: string;
}

export type ServerMessage =
  | ({ type: "state" } & Snapshot)
  | { type: "console_backlog"; lines: ConsoleLine[] }
  | { type: "console"; line: ConsoleLine }
  | { type: "console_cleared" }
  | ({ type: "info" } & Info)
  | { type: "error"; message: string };

export type ClientCommand =
  | { type: "set_team_number"; team_number: number }
  | { type: "set_use_usb"; use_usb: boolean }
  | { type: "set_game_data"; game_data: string }
  | { type: "set_mode"; mode: Mode }
  | { type: "set_alliance"; alliance: AllianceStation }
  | { type: "enable" }
  | { type: "disable" }
  | { type: "estop" }
  | { type: "restart_code" }
  | { type: "reboot_roborio" }
  | { type: "move_joystick"; from: number; to: number }
  | { type: "bind_joystick"; id: string; slot: number | null }
  | { type: "unbind_joystick"; id: string }
  | { type: "clear_console" };
