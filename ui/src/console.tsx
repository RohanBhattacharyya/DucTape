import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { send, useBackend } from "./backend";
import { ConsoleView } from "./components/ConsoleView";
import type { LineKind } from "./protocol";
import "./styles.css";

function ConsoleWindow() {
  const state = useBackend();
  const [filter, setFilter] = useState("");
  const [hidden, setHidden] = useState<Set<LineKind>>(new Set());
  const needle = filter.toLowerCase();
  const lines = state.console.filter(
    (l) => !hidden.has(l.kind) && (!needle || l.text.toLowerCase().includes(needle)),
  );
  const toggle = (kind: LineKind) => {
    const next = new Set(hidden);
    if (!next.delete(kind)) next.add(kind);
    setHidden(next);
  };

  return (
    <div className="d-flex flex-column h-100 p-2 gap-2">
      <div className="d-flex gap-2 align-items-center">
        <input
          className="form-control form-control-sm"
          placeholder="Filter"
          value={filter}
          onChange={(ev) => setFilter(ev.target.value)}
        />
        {(["stdout", "error", "warning", "ductape"] as LineKind[]).map((kind) => (
          <button
            key={kind}
            type="button"
            className={`btn btn-sm ${hidden.has(kind) ? "btn-outline-secondary" : "btn-secondary"}`}
            onClick={() => toggle(kind)}
          >
            {kind}
          </button>
        ))}
        <button type="button" className="btn btn-sm btn-outline-danger" onClick={() => send({ type: "clear_console" })}>
          Clear
        </button>
        {!state.connected && <span className="text-danger small text-nowrap">Reconnecting…</span>}
      </div>
      <ConsoleView lines={lines} className="flex-grow-1" />
    </div>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ConsoleWindow />
  </StrictMode>,
);
