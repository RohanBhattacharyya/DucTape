import { useSyncExternalStore } from "react";
import type { ClientCommand, ConsoleLine, Info, ServerMessage, Snapshot } from "./protocol";

const MAX_LINES = 5000;

export interface BackendState {
  connected: boolean;
  snapshot: Snapshot | null;
  info: Info | null;
  console: ConsoleLine[];
  error: string | null;
}

/** The WebSocket connection to the Conductor backend, with automatic reconnection. */
class Backend {
  private ws: WebSocket | null = null;
  private listeners = new Set<() => void>();
  private retryMs = 250;
  private errorTimer: number | undefined;
  state: BackendState = {
    connected: false,
    snapshot: null,
    info: null,
    console: [],
    error: null,
  };

  constructor() {
    this.connect();
  }

  private update(patch: Partial<BackendState>) {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  private connect() {
    const proto = location.protocol === "https:" ? "wss:" : "ws:";
    const ws = new WebSocket(`${proto}//${location.host}/ws`);
    this.ws = ws;
    ws.onopen = () => {
      this.retryMs = 250;
      this.update({ connected: true });
    };
    ws.onmessage = (ev) => {
      if (typeof ev.data === "string") this.handle(JSON.parse(ev.data) as ServerMessage);
    };
    ws.onclose = () => {
      this.ws = null;
      this.update({ connected: false });
      window.setTimeout(() => this.connect(), this.retryMs);
      this.retryMs = Math.min(this.retryMs * 2, 3000);
    };
  }

  private handle(msg: ServerMessage) {
    switch (msg.type) {
      case "state": {
        const { type: _, ...snapshot } = msg;
        this.update({ snapshot });
        break;
      }
      case "info": {
        const { type: _, ...info } = msg;
        this.update({ info });
        break;
      }
      case "console_backlog":
        this.update({ console: msg.lines.slice(-MAX_LINES) });
        break;
      case "console": {
        const lines = this.state.console.length >= MAX_LINES
          ? [...this.state.console.slice(1 - MAX_LINES), msg.line]
          : [...this.state.console, msg.line];
        this.update({ console: lines });
        break;
      }
      case "console_cleared":
        this.update({ console: [] });
        break;
      case "error":
        this.showError(msg.message);
        break;
    }
  }

  showError(message: string) {
    window.clearTimeout(this.errorTimer);
    this.update({ error: message });
    this.errorTimer = window.setTimeout(() => this.update({ error: null }), 5000);
  }

  send(command: ClientCommand) {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(command));
    } else {
      this.showError("Not connected to the Conductor backend");
    }
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };
}

export const backend = new Backend();

export function useBackend(): BackendState {
  return useSyncExternalStore(backend.subscribe, () => backend.state);
}

export const send = (command: ClientCommand) => backend.send(command);

/** True when running inside Conductor's native window (which provides window.ipc) */
export const nativeIpc = (): { postMessage(msg: string): void } | undefined =>
  (window as unknown as { ipc?: { postMessage(msg: string): void } }).ipc;
