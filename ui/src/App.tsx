import { useEffect, useState } from "react";
import { send, useBackend } from "./backend";
import { ControlPage } from "./pages/ControlPage";
import { ConfigPage } from "./pages/ConfigPage";
import { JoysticksPage } from "./pages/JoysticksPage";

const TABS = ["Control", "Config", "Joysticks"] as const;
type Tab = (typeof TABS)[number];

export function App() {
  const state = useBackend();
  const [tab, setTabState] = useState<Tab>(
    () => TABS.find((t) => `#${t.toLowerCase()}` === location.hash) ?? "Control",
  );
  const setTab = (t: Tab) => {
    setTabState(t);
    history.replaceState(null, "", `#${t.toLowerCase()}`);
  };
  const [warningDismissed, setWarningDismissed] = useState(false);
  const s = state.snapshot;

  // Without global hotkeys, Enter and Space only work while this window has focus
  const localKeys = s != null && !s.backend_keybinds;
  const enabled = s?.enabled ?? false;
  useEffect(() => {
    if (!localKeys) return;
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Enter") {
        send({ type: "disable" });
      } else if (ev.key === " " && enabled) {
        ev.preventDefault();
        send({ type: "estop" });
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [localKeys, enabled]);

  if (!s) {
    return (
      <div className="d-flex h-100 align-items-center justify-content-center text-body-secondary">
        {state.connected ? "Loading…" : "Connecting to DucTape…"}
      </div>
    );
  }

  return (
    <div className="d-flex flex-column h-100">
      <ul className="nav nav-tabs px-2 pt-1">
        {TABS.map((t) => (
          <li key={t} className="nav-item">
            <button type="button" className={`nav-link ${tab === t ? "active" : ""}`} onClick={() => setTab(t)}>
              {t}
            </button>
          </li>
        ))}
        <li className="ms-auto align-self-center d-flex align-items-center gap-2 pe-2 small">
          {!state.connected && <span className="text-danger">Reconnecting…</span>}
          <img src="/favicon.svg" alt="" width={22} height={22} />
          <span className="text-body-secondary">DucTape</span>
        </li>
      </ul>

      {localKeys && !warningDismissed && (
        <div className="alert alert-warning alert-dismissible py-2 mx-3 mt-2 mb-0 small">
          <strong>Global hotkeys unavailable.</strong> Enter (disable) and Space (e-stop) only work while this
          window is focused.
          <button type="button" className="btn-close py-2" aria-label="Dismiss" onClick={() => setWarningDismissed(true)} />
        </div>
      )}
      {s.link_error && (
        <div className="alert alert-danger py-2 mx-3 mt-2 mb-0 small">
          <strong>Can't reach any robot:</strong> {s.link_error}. Close the other driver station.
        </div>
      )}
      {state.error && <div className="alert alert-danger py-2 mx-3 mt-2 mb-0 small">{state.error}</div>}

      <div className="flex-grow-1 overflow-auto">
        {tab === "Control" && <ControlPage state={state} />}
        {tab === "Config" && <ConfigPage state={state} />}
        {tab === "Joysticks" && <JoysticksPage state={state} />}
      </div>
    </div>
  );
}
