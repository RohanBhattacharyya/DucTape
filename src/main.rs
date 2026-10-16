use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context;
use clap::{Args, Parser, Subcommand};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

mod config;
mod console;
mod controller;
#[cfg(feature = "gui")]
mod gui;
mod joystick;
#[cfg(feature = "gui")]
mod keys;
mod mcp;
mod panic;
mod protocol;
mod server;

use config::EnablePolicy;
use controller::{Controller, Options};

const DEFAULT_MCP_ADDR: &str = "127.0.0.1:7341";

/// DucTape Driver Station: a driver station for FRC robots, with a Model Context Protocol
/// server for AI agents. A revival of Conductor.
#[derive(Parser)]
#[command(name = "ductape", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Run an MCP server on stdin/stdout, for agents like Claude Code and Claude Desktop.
    ///
    /// Runs headless unless --gui is given.
    Mcp {
        /// Also show the driver station windows, so you can watch the agent and take over
        #[arg(long)]
        gui: bool,
    },
    /// Print how to connect MCP clients to DucTape
    McpSetup,
}

#[derive(Args, Clone)]
struct RunArgs {
    /// Config file to use instead of the default location
    #[arg(long, value_name = "PATH", global = true, env = "DUCTAPE_CONFIG")]
    config: Option<std::path::PathBuf>,

    /// Team number (saved for next time)
    #[arg(long, short, global = true, env = "DUCTAPE_TEAM")]
    team: Option<u32>,

    /// Connect over the roboRIO's USB interface (172.22.11.2)
    #[arg(long, global = true)]
    usb: bool,

    /// Connect to this address instead of 10.TE.AM.2
    #[arg(long, value_name = "IP", global = true)]
    robot_ip: Option<IpAddr>,

    /// Don't open windows; serve the UI for a browser instead
    #[arg(long, global = true)]
    headless: bool,

    /// Address for the UI and its WebSocket. Port 0 picks a free port
    #[arg(
        long,
        value_name = "ADDR",
        default_value = "127.0.0.1:0",
        global = true,
        env = "DUCTAPE_UI_ADDR"
    )]
    ui_addr: SocketAddr,

    /// Serve MCP over Streamable HTTP at http://ADDR/mcp, so agents can attach to this window
    #[arg(long, value_name = "ADDR", num_args = 0..=1, default_missing_value = DEFAULT_MCP_ADDR, global = true)]
    mcp_http: Option<SocketAddr>,

    /// Restrict when MCP clients may enable the robot [default: always]
    #[arg(
        long,
        value_enum,
        value_name = "POLICY",
        global = true,
        env = "DUCTAPE_MCP_ENABLE"
    )]
    mcp_enable: Option<EnablePolicy>,

    /// Time-limit MCP enables to this many seconds [default: untimed]
    #[arg(long, value_name = "SECONDS", global = true)]
    mcp_max_enable: Option<u64>,

    /// Don't capture Enter/Space system-wide (the window still handles them when focused)
    #[arg(long, global = true)]
    no_global_hotkeys: bool,
}

fn init_logging() {
    let filter = EnvFilter::try_from_env("DUCTAPE_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info,ds=info"));
    // Always stderr: stdout carries the MCP protocol in `ductape mcp`
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn mcp_setup() {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "ductape".into());
    println!(
        r#"DucTape speaks the Model Context Protocol, so AI agents can run the driver station.

Claude Code (headless, or add --gui to watch and take over):
    claude mcp add ductape -- {exe} mcp

Claude Desktop, Cursor, VS Code and other clients (JSON config):
    {{
      "mcpServers": {{
        "ductape": {{ "command": "{exe}", "args": ["mcp", "--gui"] }}
      }}
    }}

Attach an agent to a DucTape window you already have open:
    {exe} --mcp-http
    claude mcp add --transport http ductape http://{DEFAULT_MCP_ADDR}/mcp

Agents have full control by default, and their enables last until disabled.
To restrict them, pass --mcp-enable simulator (only the WPILib simulator) or
--mcp-enable never, and --mcp-max-enable SECONDS to time-limit their enables.
These can also be set under [mcp] in {config}."#,
        config = config::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "the config file".into()),
    );
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(path) = &cli.run.config {
        // SAFETY: still single-threaded; config::path reads this
        unsafe { std::env::set_var("DUCTAPE_CONFIG", path) };
    }
    if matches!(cli.command, Some(Command::McpSetup)) {
        mcp_setup();
        return ExitCode::SUCCESS;
    }
    init_logging();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    let args = cli.run;
    let stdio_mcp = matches!(cli.command, Some(Command::Mcp { .. }));
    let want_gui = match cli.command {
        Some(Command::Mcp { gui }) => gui && !args.headless,
        _ => !args.headless,
    };
    #[cfg(not(feature = "gui"))]
    if want_gui && !stdio_mcp {
        tracing::warn!("this build has no GUI support, running headless");
    }
    #[cfg(target_os = "windows")]
    if want_gui {
        tinyfiledialogs::message_box_ok(
            "Unsupported Environment",
            "The DucTape Driver Station is not supported on Windows. Please use the NI Driver Station instead.",
            tinyfiledialogs::MessageBoxIcon::Error,
        );
        anyhow::bail!("Windows is not supported");
    }
    if want_gui {
        panic::install();
    }

    let mut cfg = config::load();
    if let Some(team) = args.team {
        anyhow::ensure!(
            team <= controller::MAX_TEAM_NUMBER,
            "team numbers go up to {}",
            controller::MAX_TEAM_NUMBER
        );
        cfg.team_number = team;
        config::store(&cfg);
    }
    let mut opts = Options::from_config(&cfg);
    opts.use_usb |= args.usb;
    opts.robot_ip = args.robot_ip;
    if let Some(policy) = args.mcp_enable {
        opts.enable_policy = policy;
    }
    if let Some(secs) = args.mcp_max_enable {
        opts.max_enable = Some(Duration::from_secs(secs.max(1)));
    }
    let mcp_http = match args.mcp_http {
        Some(addr) => Some(addr),
        None => cfg
            .mcp
            .http_addr
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("invalid mcp.http_addr in config")?,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ductape")
        .build()?;
    let _guard = runtime.enter();

    let ctrl = Controller::start(opts, cfg);
    let cancel = CancellationToken::new();
    let servers = runtime.block_on(server::start(
        ctrl.clone(),
        args.ui_addr,
        mcp_http,
        cancel.clone(),
    ))?;
    tracing::info!("UI at {}", servers.ui_url());
    if let Some(url) = servers.mcp_url() {
        tracing::info!("MCP (Streamable HTTP) at {url}");
    }
    tracing::info!("MCP enable policy: {:?}", ctrl.enable_policy());

    let stdio = stdio_mcp.then(|| runtime.spawn(mcp::serve_stdio(ctrl.clone())));

    let result = if want_gui {
        run_gui(
            &runtime,
            &ctrl,
            &servers,
            stdio,
            !stdio_mcp && !args.no_global_hotkeys,
        )
    } else {
        if stdio.is_none() {
            eprintln!(
                "DucTape is running headless. Open {} in a browser.",
                servers.ui_url()
            );
        }
        runtime.block_on(async {
            match stdio {
                Some(task) => tokio::select! {
                    res = task => res.context("MCP server crashed")?,
                    _ = tokio::signal::ctrl_c() => Ok(()),
                },
                None => {
                    tokio::signal::ctrl_c().await?;
                    Ok(())
                }
            }
        })
    };

    ctrl.shutdown();
    cancel.cancel();
    runtime.shutdown_timeout(Duration::from_secs(1));
    result
}

#[cfg(feature = "gui")]
fn run_gui(
    runtime: &tokio::runtime::Runtime,
    ctrl: &Controller,
    servers: &server::Servers,
    stdio: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    global_hotkeys: bool,
) -> anyhow::Result<()> {
    let keybinds = global_hotkeys && keys::bind_keys(ctrl.clone());
    ctrl.set_backend_keybinds(keybinds);

    gui::run(&servers.ui_url(), |handle| {
        // Close the windows when the MCP client goes away or on Ctrl+C
        runtime.spawn(async move {
            match stdio {
                Some(task) => {
                    tokio::select! {
                        _ = task => {}
                        _ = tokio::signal::ctrl_c() => {}
                    }
                }
                None => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
            handle.quit();
        });
    })
}

#[cfg(not(feature = "gui"))]
fn run_gui(
    runtime: &tokio::runtime::Runtime,
    _ctrl: &Controller,
    servers: &server::Servers,
    stdio: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    _global_hotkeys: bool,
) -> anyhow::Result<()> {
    eprintln!("Open {} in a browser.", servers.ui_url());
    runtime.block_on(async {
        match stdio {
            Some(task) => tokio::select! {
                res = task => res.context("MCP server crashed")?,
                _ = tokio::signal::ctrl_c() => Ok(()),
            },
            None => Ok(tokio::signal::ctrl_c().await?),
        }
    })
}
