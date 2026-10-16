import { useEffect, useState } from "react";
import { send } from "../backend";
import type { BackendState } from "../backend";

const POLICY_TEXT = {
  never: "Agents cannot enable the robot (--mcp-enable never).",
  simulator: "Agents may enable the WPILib simulator only (--mcp-enable simulator).",
  always: "Agents have full control, including enabling real robots. Keep the e-stop within reach!",
};

/** A text field that applies its value on Enter or when it loses focus */
function useDraft(value: string) {
  const [draft, setDraft] = useState(value);
  const [editing, setEditing] = useState(false);
  useEffect(() => {
    if (!editing) setDraft(value);
  }, [value, editing]);
  return { draft, setDraft, setEditing };
}

export function ConfigPage({ state }: { state: BackendState }) {
  const s = state.snapshot!;
  const info = state.info;
  const team = useDraft(String(s.team_number));
  const gsm = useDraft(s.game_data);

  const applyTeam = () => {
    team.setEditing(false);
    const n = Number(team.draft);
    if (Number.isInteger(n) && n >= 0 && n !== s.team_number) {
      send({ type: "set_team_number", team_number: n });
    }
  };
  const applyGsm = () => {
    gsm.setEditing(false);
    if (gsm.draft !== s.game_data) send({ type: "set_game_data", game_data: gsm.draft });
  };

  return (
    <div className="container-fluid pt-3">
      <div className="row g-4">
        <div className="col-4">
          <label htmlFor="team" className="form-label">
            Team Number
          </label>
          <input
            id="team"
            type="number"
            min={0}
            max={25599}
            className="form-control mb-1"
            value={team.draft}
            onFocus={() => team.setEditing(true)}
            onChange={(ev) => ev.target.value.length <= 5 && team.setDraft(ev.target.value)}
            onKeyDown={(ev) => ev.key === "Enter" && applyTeam()}
            onBlur={applyTeam}
          />
          <div className="form-text mb-3">Connecting to {s.target_address}</div>

          <div className="form-check form-switch mb-3">
            <input
              id="usb"
              type="checkbox"
              role="switch"
              className="form-check-input"
              checked={s.use_usb}
              onChange={(ev) => send({ type: "set_use_usb", use_usb: ev.target.checked })}
            />
            <label htmlFor="usb" className="form-check-label">
              Connect via USB (172.22.11.2)
            </label>
          </div>

          <label htmlFor="gsm" className="form-label">
            Game Data
          </label>
          <input
            id="gsm"
            type="text"
            maxLength={3}
            className="form-control"
            value={gsm.draft}
            onFocus={() => gsm.setEditing(true)}
            onChange={(ev) => gsm.setDraft(ev.target.value)}
            onKeyDown={(ev) => ev.key === "Enter" && applyGsm()}
            onBlur={applyGsm}
          />
        </div>

        <div className="col-3 d-flex align-items-start">
          <div className="btn-group-vertical">
            <button type="button" className="btn btn-secondary" onClick={() => send({ type: "reboot_roborio" })}>
              Restart roboRIO
            </button>
            <button type="button" className="btn btn-secondary" onClick={() => send({ type: "restart_code" })}>
              Restart Robot Code
            </button>
          </div>
        </div>

        <div className="col">
          <div className="card">
            <div className="card-body small">
              <h6 className="card-title">AI agents (MCP)</h6>
              {info && (
                <>
                  <p className="mb-2">
                    {POLICY_TEXT[info.mcp_enable_policy]}{" "}
                    {info.mcp_max_enable_secs == null
                      ? "Agent enables last until disabled, unless the agent asks for a time limit."
                      : `Agent enables are disabled after at most ${info.mcp_max_enable_secs}s.`}
                  </p>
                  {info.mcp_url ? (
                    <p className="mb-2">
                      Listening at <code className="user-select-all">{info.mcp_url}</code>
                      <br />
                      <code className="user-select-all">
                        claude mcp add --transport http ductape {info.mcp_url}
                      </code>
                    </p>
                  ) : (
                    <p className="mb-2">
                      Launch with <code>ductape --mcp-http</code> to let agents attach to this window, or
                      run <code>ductape mcp-setup</code> for other options.
                    </p>
                  )}
                </>
              )}
              <p className="mb-0 text-body-secondary">
                Enter disables and Space e-stops at any time{s.backend_keybinds ? ", even when this window is not focused" : " while this window is focused"}.
              </p>
            </div>
          </div>
          {info && (
            <div className="text-body-secondary small mt-2">
              DucTape Driver Station {info.version}, a revival of{" "}
              <a href="https://github.com/Redrield/Conductor" target="_blank" rel="noreferrer">
                Conductor
              </a>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
