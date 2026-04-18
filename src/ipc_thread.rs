//! Background thread that owns the niri IPC socket connection.
//!
//! The socket client API (`niri_ipc::socket::Socket::read_events`) returns
//! a blocking `FnMut() -> io::Result<Event>`. GTK4 requires all UI work to
//! happen on the main thread, so we spawn a std::thread, loop reading
//! events, filter down to `Event::RecognitionFrame`, and forward the
//! payload to the main loop via a `glib::MainContext::channel`.
//!
//! On IO errors (socket closed, niri restarted, JSON decode failure) the
//! thread sends a `Disconnect` message and exits. The UI can later spawn
//! a fresh thread to reconnect — currently we just display the
//! disconnected state and the user restarts the inspector.

use anyhow::{Context, Result};
use niri_ipc::socket::Socket;
use niri_ipc::{Event, Request, Response};
use std::thread;

/// Snapshot of one recognizer frame, forwarded to the UI thread.
///
/// This is `niri_ipc::Event::RecognitionFrame`'s fields flattened into
/// an owned struct so the UI doesn't need to care about the enum shape.
/// If the niri-ipc event gains/renames fields, this struct and the
/// conversion below are the single adapt site.
#[allow(dead_code)] // timestamp_ms is reserved for the future sparkline view
#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    pub finger_count: u8,
    pub swipe_distance: f64,
    pub swipe_trigger_distance: f64,
    pub spread_change: f64,
    pub pinch_trigger_distance: f64,
    pub rotation_rad: f64,
    pub rotation_trigger_angle_rad: f64,
    pub rotation_arc: f64,
    pub rotation_arc_trigger_distance: f64,
    pub is_rotate: bool,
    pub is_pinch: bool,
    pub closest: String,
    pub timestamp_ms: u32,
}

/// Messages sent from the IPC thread to the UI thread.
#[derive(Debug, Clone)]
pub enum IpcMsg {
    /// Connection is up and we're reading events.
    Connected,
    /// A new recognition frame arrived.
    Frame(FrameSnapshot),
    /// A tagged gesture just latched — useful to annotate the scope
    /// history with "this is where niri committed to a classification".
    GestureLocked {
        tag: String,
        trigger: String,
        finger_count: u8,
    },
    /// A tagged gesture ended.
    GestureEnded { tag: String, completed: bool },
    /// The socket read loop exited — either clean EOF or an error.
    /// UI should show "disconnected" and stop animating.
    Disconnected(String),
}

/// Spawn the IPC reader thread. Returns immediately; the thread runs
/// until either the socket closes or an unrecoverable error.
pub fn spawn<F>(send: F)
where
    F: Fn(IpcMsg) + Send + 'static,
{
    thread::Builder::new()
        .name("niri-ipc-reader".into())
        .spawn(move || {
            let err = match run_loop(&send) {
                Ok(()) => "clean EOF from niri socket".to_string(),
                Err(e) => format!("{:#}", e),
            };
            send(IpcMsg::Disconnected(err));
        })
        .expect("spawn ipc thread");
}

fn run_loop<F: Fn(IpcMsg)>(send: &F) -> Result<()> {
    // Connect to the well-known niri socket (NIRI_SOCKET env var, or
    // $XDG_RUNTIME_DIR/niri.<wayland-display>.sock as fallback — the
    // niri-ipc crate handles this lookup for us).
    let mut socket =
        Socket::connect().context("failed to connect to niri IPC socket (is niri running?)")?;

    // Switch the socket into event-stream mode. niri stops reading
    // further Request messages after this and starts pushing Events.
    let reply = socket
        .send(Request::EventStream)
        .context("failed to send Request::EventStream")?;

    match reply {
        Ok(Response::Handled) => {}
        Ok(other) => anyhow::bail!("unexpected response to EventStream: {other:?}"),
        Err(e) => anyhow::bail!("niri rejected EventStream: {e}"),
    }

    send(IpcMsg::Connected);

    // Blocking read loop. read_events() consumes the socket and
    // returns a closure that yields the next event each call.
    let mut read_event = socket.read_events();
    loop {
        let event = read_event().context("error reading event from niri socket")?;

        match event {
            Event::RecognitionFrame {
                finger_count,
                swipe_distance,
                swipe_trigger_distance,
                spread_change,
                pinch_trigger_distance,
                rotation_rad,
                rotation_trigger_angle_rad,
                rotation_arc,
                rotation_arc_trigger_distance,
                is_rotate,
                is_pinch,
                closest,
                timestamp_ms,
            } => {
                send(IpcMsg::Frame(FrameSnapshot {
                    finger_count,
                    swipe_distance,
                    swipe_trigger_distance,
                    spread_change,
                    pinch_trigger_distance,
                    rotation_rad,
                    rotation_trigger_angle_rad,
                    rotation_arc,
                    rotation_arc_trigger_distance,
                    is_rotate,
                    is_pinch,
                    closest,
                    timestamp_ms,
                }));
            }
            Event::GestureBegin {
                tag,
                trigger,
                finger_count,
                ..
            } => {
                send(IpcMsg::GestureLocked {
                    tag,
                    trigger,
                    finger_count,
                });
            }
            Event::GestureEnd { tag, completed } => {
                send(IpcMsg::GestureEnded { tag, completed });
            }
            // All other events are irrelevant for the inspector; drop them.
            _ => {}
        }
    }
}
