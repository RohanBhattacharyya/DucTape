//! Native windows: the main driver station window and the robot console window.

use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy, EventLoopWindowTarget};
use tao::platform::run_return::EventLoopExtRunReturn;
use tao::window::{Window, WindowBuilder, WindowId};
use wry::{WebView, WebViewBuilder};

/// Fraction of the monitor the main window covers, as in Conductor 0.1
const WIDTH_FRACTION: f64 = 0.79;
const HEIGHT_FRACTION: f64 = 0.42;

#[derive(Debug)]
pub enum UserEvent {
    OpenConsole,
    Quit,
}

/// Lets other threads close the GUI
#[derive(Clone)]
pub struct GuiHandle(EventLoopProxy<UserEvent>);

impl GuiHandle {
    pub fn quit(&self) {
        let _ = self.0.send_event(UserEvent::Quit);
    }
}

struct Pane {
    window: Window,
    _webview: WebView,
}

fn open(
    target: &EventLoopWindowTarget<UserEvent>,
    proxy: &EventLoopProxy<UserEvent>,
    title: &str,
    url: &str,
    size: LogicalSize<f64>,
) -> anyhow::Result<Pane> {
    let window = WindowBuilder::new()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size(LogicalSize::new(640.0, 360.0))
        .build(target)?;

    let proxy = proxy.clone();
    let builder = WebViewBuilder::new()
        .with_url(url)
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |req| {
            if req.body() == "open-console" {
                let _ = proxy.send_event(UserEvent::OpenConsole);
            }
        });

    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .ok_or_else(|| anyhow::anyhow!("window has no GTK container"))?;
        builder.build_gtk(vbox)?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(&window)?;

    Ok(Pane {
        window,
        _webview: webview,
    })
}

/// Runs the windows on the current (main) thread. `on_ready` receives a handle once the event
/// loop exists. Returns when the main window is closed or [`GuiHandle::quit`] is called.
pub fn run(ui_url: &str, on_ready: impl FnOnce(GuiHandle)) -> anyhow::Result<()> {
    let mut event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let main_size = event_loop
        .primary_monitor()
        .map(|m| {
            let size = m.size().to_logical::<f64>(m.scale_factor());
            LogicalSize::new(
                (size.width * WIDTH_FRACTION).max(960.0),
                (size.height * HEIGHT_FRACTION).max(420.0),
            )
        })
        .unwrap_or(LogicalSize::new(1100.0, 460.0));

    let main = open(&event_loop, &proxy, "Conductor DS", ui_url, main_size)?;
    let main_id = main.window.id();
    let console_url = format!("{ui_url}/console");
    let console_size = LogicalSize::new(650.0, 650.0);
    let mut console: Option<Pane> = Some(open(
        &event_loop,
        &proxy,
        "Robot Console",
        &console_url,
        console_size,
    )?);

    on_ready(GuiHandle(proxy.clone()));

    let mut main = Some(main);
    event_loop.run_return(|event, target, control_flow| {
        *control_flow = ControlFlow::Wait;
        let close = |id: WindowId, console: &mut Option<Pane>| {
            if console.as_ref().is_some_and(|c| c.window.id() == id) {
                *console = None;
                false
            } else {
                id == main_id
            }
        };
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
                ..
            } => {
                if close(window_id, &mut console) {
                    main = None;
                    console = None;
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(UserEvent::OpenConsole) => match &console {
                Some(pane) => pane.window.set_focus(),
                None => match open(target, &proxy, "Robot Console", &console_url, console_size) {
                    Ok(pane) => console = Some(pane),
                    Err(e) => tracing::warn!("could not open console window: {e}"),
                },
            },
            Event::UserEvent(UserEvent::Quit) => {
                main = None;
                console = None;
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
    Ok(())
}
