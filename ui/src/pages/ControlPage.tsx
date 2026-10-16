import { useState } from "react";
import { nativeIpc, send } from "../backend";
import type { BackendState } from "../backend";
import { ConsoleView } from "../components/ConsoleView";
import { MODE_LABELS, MODES } from "../protocol";
import type { AllianceColor, Snapshot } from "../protocol";

const EXPLANATIONS = {
  comms: ["The robot controller and driver station are not able to communicate."],
  code: [
    "There is no user code running on the robot.",
    "1. Code may be crashing on startup, check robot console for potential error.",
    "2. There may be no code downloaded. Deploy your code to the robot.",
  ],
  joysticks: [
    "No joysticks were identified",
    "1. Ensure they are plugged in",
    "2. Disconnect and reconnect the joysticks",
  ],
};
type Explanation = keyof typeof EXPLANATIONS;

const STATIONS: { color: AllianceColor; station: number }[] = [
  { color: "red", station: 1 },
  { color: "red", station: 2 },
  { color: "red", station: 3 },
  { color: "blue", station: 1 },
  { color: "blue", station: 2 },
  { color: "blue", station: 3 },
];

function voltageClass(v: number) {
  if (v < 8.5) return "text-danger";
  if (v <= 11.5) return "text-warning";
  return "text-success";
}

function Telemetry({
  name,
  ok,
  explain,
  onExplain,
}: {
  name: string;
  ok: boolean;
  explain: Explanation;
  onExplain: (e: Explanation | null) => void;
}) {
  return (
    <li
      className="list-group-item d-flex justify-content-between align-items-center py-2"
      onMouseEnter={() => !ok && onExplain(explain)}
      onMouseLeave={() => onExplain(null)}
    >
      {name}
      <span className={`indicator ${ok ? "ok" : ""}`} title={ok ? "OK" : "Hover for details"} />
    </li>
  );
}

function statusLines(s: Snapshot) {
  if (s.estopped || !s.comms || !s.code) return s.status;
  return `${MODE_LABELS[s.mode]}\n${s.enabled ? "Enabled" : "Disabled"}`;
}

export function ControlPage({ state }: { state: BackendState }) {
  const s = state.snapshot!;
  const [explanation, setExplanation] = useState<Explanation | null>(null);
  const ipc = nativeIpc();
  const agentDriving = s.enabled && s.last_enable_change_by === "mcp";

  const blur = (ev: React.MouseEvent<HTMLButtonElement>) => ev.currentTarget.blur();

  return (
    <div className="container-fluid pt-3">
      <div className="row g-3">
        <div className="col-3 d-flex flex-column gap-3">
          <div className="btn-group-vertical mode-list" role="group" aria-label="Mode">
            {MODES.map((mode) => (
              <button
                key={mode}
                type="button"
                className={`btn btn-secondary border border-dark ${s.mode === mode ? "active" : ""}`}
                onClick={(ev) => {
                  blur(ev);
                  send({ type: "set_mode", mode });
                }}
              >
                {MODE_LABELS[mode]}
              </button>
            ))}
          </div>
          <div className="btn-group" role="group" aria-label="Enable">
            <button
              type="button"
              className={`btn btn-lg btn-secondary enable-btn ${s.enabled ? "active" : ""}`}
              disabled={s.estopped}
              onClick={(ev) => {
                blur(ev);
                send({ type: "enable" });
              }}
            >
              Enable
            </button>
            <button
              type="button"
              className={`btn btn-lg btn-secondary disable-btn ${!s.enabled ? "active" : ""}`}
              onClick={(ev) => {
                blur(ev);
                send({ type: "disable" });
              }}
            >
              Disable
            </button>
          </div>
        </div>

        <div className="col-3 d-flex flex-column gap-3">
          <ul className="list-group">
            <Telemetry name="Communications" ok={s.comms} explain="comms" onExplain={setExplanation} />
            <Telemetry name="Robot Code" ok={s.code} explain="code" onExplain={setExplanation} />
            <Telemetry
              name="Joysticks"
              ok={s.joysticks.some((j) => j.device)}
              explain="joysticks"
              onExplain={setExplanation}
            />
          </ul>
          <div className="d-flex align-items-center gap-2">
            <label htmlFor="station" className="lead mb-0 text-nowrap">
              Team Station
            </label>
            <select
              id="station"
              className="form-select"
              value={`${s.alliance.color}-${s.alliance.station}`}
              onChange={(ev) => {
                const [color, station] = ev.target.value.split("-");
                send({
                  type: "set_alliance",
                  alliance: { color: color as AllianceColor, station: Number(station) },
                });
              }}
            >
              {STATIONS.map((st) => (
                <option key={`${st.color}-${st.station}`} value={`${st.color}-${st.station}`}>
                  {st.color === "red" ? "Red" : "Blue"} {st.station}
                </option>
              ))}
            </select>
          </div>
        </div>

        <div className="col-2 text-center">
          <p className="lead mb-2">Team # {s.team_number}</p>
          <p className={`fs-4 fw-bold mb-2 ${voltageClass(s.voltage)}`}>
            {s.voltage.toFixed(2)}V{s.brownout && <span className="badge bg-warning ms-2 fs-6">Brownout</span>}
          </p>
          <p className="status-text mb-2">{statusLines(s)}</p>
          {agentDriving && (
            <span className="badge agent-badge mb-1" title="An MCP client (AI agent) enabled the robot">
              Enabled by AI agent
            </span>
          )}
          {s.auto_disable_in_ms != null && (
            <div className="small text-warning">
              Auto-disable in {(s.auto_disable_in_ms / 1000).toFixed(1)}s
            </div>
          )}
        </div>

        <div className="col d-flex flex-column">
          <ConsoleView
            lines={state.console}
            override={explanation ? EXPLANATIONS[explanation] : null}
            style={{ height: "220px" }}
          />
          <div className="d-flex justify-content-between align-items-center mt-2 small">
            <div>
              {ipc && (
                <button
                  type="button"
                  className="btn btn-sm btn-outline-secondary me-2"
                  onClick={() => ipc.postMessage("open-console")}
                >
                  Open console window
                </button>
              )}
              <button
                type="button"
                className="btn btn-sm btn-outline-secondary"
                onClick={() => send({ type: "clear_console" })}
              >
                Clear
              </button>
            </div>
            <span>
              Simulator <span className={`indicator round ${s.simulator ? "ok" : ""}`} />
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
