# Archived

Starting in 2027, the official Driver Station has cross platform support, I no longer recommend using Conductor in any scenario.

More information can be found [here](https://wpilib.org/blog/the-2027-first-driver-station) and [here](https://github.com/wpilibsuite/FirstDriverStation-Public)
<br/><br/><br/><br/><br/>

[![CI](https://github.com/Redrield/Conductor/actions/workflows/ci.yml/badge.svg)](https://github.com/Redrield/Conductor/actions/workflows/ci.yml)

# Conductor: A cross-platform FRC Driver Station

Conductor is a Driver Station for FRC robots for Linux and macOS which supports nearly<sup>*</sup> every feature of the NI Driver Station. It also includes a [Model Context Protocol](https://modelcontextprotocol.io) server, so AI agents such as Claude can run the driver station too: check the robot's status, run timed tests in the WPILib simulator, read the console and drive a virtual joystick.

<sup>*</sup>Conductor cannot yet auto-launch dashboards, and is not and will never be compatible with the Field Management System.

- [Installing](#installing)
- [Usage](#usage)
- [AI agents (MCP)](#ai-agents-mcp)
- [Command line](#command-line)
- [Building and contributing](#building-and-contributing)

# Installing

## Linux packages

Every [release](https://github.com/Redrield/Conductor/releases) has packages for x86_64 and aarch64:

| Distribution | Install |
| --- | --- |
| Debian, Ubuntu 22.04+, Mint, Pop!_OS | `sudo apt install ./conductor_*.deb` |
| Fedora, RHEL, openSUSE | `sudo dnf install ./conductor-*.rpm` |
| Arch, Manjaro, CachyOS | `makepkg -si` with [`packaging/arch/PKGBUILD`](packaging/arch/PKGBUILD) |
| Anything else | `tar -xzf conductor-*-linux.tar.gz -C ~/.local --strip-components=1` |

Packages install a desktop entry, so Conductor shows up in your application menu.

## From source

The install script installs build dependencies (apt, dnf, pacman or zypper), a Rust toolchain if yours is missing or older than 1.88, and a private copy of Node.js if yours is older than 20.19. It then builds Conductor and installs it into `~/.local`:

```sh
curl -fsSL https://raw.githubusercontent.com/Redrield/Conductor/next/scripts/install.sh | sh
# or from a checkout, optionally system-wide:
./scripts/install.sh --system
```

Or do it by hand. You need Rust 1.88+, Node.js 20.19+ and the WebKitGTK 4.1 development files (`libwebkit2gtk-4.1-dev libgtk-3-dev libudev-dev libx11-dev` on Debian/Ubuntu):

```sh
git clone https://github.com/Redrield/Conductor && cd Conductor
make release            # or just `cargo build --release`; it builds the web UI too
sudo make install       # PREFIX=/usr/local by default
```

`cargo install --git https://github.com/Redrield/Conductor --locked` also works.

## macOS

Download `Conductor-*-macos-aarch64.zip` from the releases, or build it yourself with `cargo install cargo-bundle && cargo bundle --release`. Global hotkeys need the Input Monitoring permission (System Settings → Privacy & Security).

## Headless

Conductor can run without windows (`conductor --headless`) and serve its UI to a browser. Builds with `--no-default-features` don't need WebKitGTK at all, for example on a Raspberry Pi.

# Usage

Conductor is designed to emulate the design of the NI Driver Station as closely as possible. The main window is split into three sections, Control, Config, and Joysticks, while the second window serves as a larger viewport to see riolog messages.

Enter disables the robot and Space e-stops it. On X11 these work even when Conductor isn't focused, like the NI Driver Station; on Wayland, which doesn't allow global hotkeys, they work while a Conductor window is focused.

## Control Tab

![Control Tab](./images/Conductor.png)

*The control tab of Conductor when not connected to a robot*

![Control with Connection](./images/conductor-control-connected.png)

*The control tab of Conductor when connected to a robot*

On the far left are selectors for the mode, and buttons to enable or disable the robot. In the middle are indicators for communication, robot code and joysticks (hover a red one for an explanation), the team station, team number, battery voltage and the robot's status. On the right is a smaller view of the robot console.

The simulator indicator lights up when Conductor is connected to a WPILib simulation on the local machine instead of a robot.

![Conductor with simulator](./images/conductor-simulator.png)

*The control tab of Conductor with a connection to the WPILib Simulator*

## Config

![Configuration](./images/conductor-config.png)

The team number sets the address Conductor connects to, the familiar 10.TE.AM.2. "Connect via USB" overrides this to reach the roboRIO's USB interface at `172.22.11.2`. Game data sets the Game Specific Message given to robot code. On the right are buttons to restart user code and reboot the roboRIO. Settings are saved to `~/.config/conductor/conductor.toml`.

## Joysticks

![Joysticks Tab](./images/conductor-joysticks.png)

Robot code sees six joystick ports. Controllers are put on the first free port when they're plugged in; drag ports to reorder them, or move devices on and off ports. The Virtual Joystick can be put on a port to silence WPILib warnings about unplugged joysticks, and is what AI agents use to drive. Unplugging a controller that is on a port disables the robot.

# AI agents (MCP)

Conductor is an MCP server. Agents get tools to read status, set the team number, mode, alliance and game data, enable/disable/e-stop, restart code, reboot the roboRIO, wait for the robot to connect, read the console, manage joystick ports and drive the virtual joystick. The `run_mode` tool runs a mode for a set time and returns the console output, which makes it easy to ask an agent to "run auto in the simulator and tell me why it crashes".

Run `conductor mcp-setup` to print these instructions with the right paths.

**Claude Code**

```sh
claude mcp add conductor -- conductor mcp          # headless
claude mcp add conductor -- conductor mcp --gui    # with windows, so you can watch and take over
```

**Claude Desktop, Cursor, VS Code, and other clients**

```json
{
  "mcpServers": {
    "conductor": { "command": "conductor", "args": ["mcp", "--gui"] }
  }
}
```

**Attaching to a Conductor window you already have open**

Only one driver station can talk to a robot at a time. To let an agent use the window you're already running, start it with `--mcp-http` and connect over Streamable HTTP:

```sh
conductor --mcp-http
claude mcp add --transport http conductor http://127.0.0.1:7341/mcp
```

## Safety

Robots are dangerous, and agents make mistakes.

- **Agents may only enable the WPILib simulator by default.** To let them enable real robots, pass `--mcp-enable always` or set `enable_policy = "always"` under `[mcp]` in the config file. `--mcp-enable never` blocks agents from enabling anything.
- **Every agent enable is timed.** Conductor disables the robot when the time runs out (150 seconds at most, a full match; change with `--mcp-max-enable`).
- Enabling needs communication and running robot code, and changing mode disables the robot, as with the NI Driver Station.
- Disable and e-stop are always available to agents and to you. The window shows when an agent enabled the robot, with a countdown.
- In `conductor mcp` modes, global hotkeys are off so that typing to your agent can't e-stop the robot; Enter and Space still work in Conductor's window.
- Conductor disables the robot when communication drops, so it doesn't come back enabled.
- The HTTP endpoint only listens on localhost and rejects requests from web pages.

# Command line

```
conductor [OPTIONS]              open the driver station
conductor mcp [--gui]            MCP server on stdin/stdout
conductor mcp-setup              print MCP client configuration

  -t, --team <TEAM>              team number (saved)
      --usb                      connect over USB (172.22.11.2)
      --robot-ip <IP>            connect to this address instead
      --headless                 no windows; serve the UI to a browser
      --ui-addr <ADDR>           address for the UI server [default: 127.0.0.1:0]
      --mcp-http [<ADDR>]        also serve MCP over HTTP [default: 127.0.0.1:7341]
      --mcp-enable <POLICY>      never | simulator | always
      --mcp-max-enable <SECONDS> longest agent enable [default: 150]
      --no-global-hotkeys        only handle Enter/Space in Conductor's window
      --config <PATH>            use another config file
```

Logging goes to stderr; set `CONDUCTOR_LOG=debug` for more.

# Building and contributing

| Path | What |
| --- | --- |
| `src/` | The application: `controller.rs` owns all driver station state, `mcp.rs` is the MCP server, `server.rs` serves the UI, `gui.rs` the native windows |
| `crates/ds/` | The FRC driver station protocol library (`ds`, ported to tokio 1), with a fake roboRIO for tests |
| `ui/` | The web UI: React 19, TypeScript, Vite |
| `packaging/` | Desktop entry, AppStream metadata, icons, PKGBUILD |

```sh
make debug     # or `cargo build`; build.rs runs `npm ci` and builds the UI when it changes
make run
make dev       # hot-reloading UI on http://localhost:5173 against a headless backend
make test      # Rust tests, including end-to-end MCP tests against a fake roboRIO
make lint      # rustfmt, clippy and TypeScript checks
```

Set `CONDUCTOR_SKIP_UI_BUILD=1` to skip the npm step when you're only changing Rust. Without npm installed, Conductor embeds a placeholder page and everything else, including MCP, still works.

Releases are built by GitHub Actions when a `v*` tag is pushed.
