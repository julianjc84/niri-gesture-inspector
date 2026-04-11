//! niri-gesture-inspector
//!
//! Live GTK4 visualizer for the touchscreen gesture recognizer in niri.
//! Subscribes to the niri IPC event stream, consumes the
//! `Event::RecognitionFrame` variant emitted on debug builds, and renders
//! threshold bars + classifier flags + recent-lock history so you can see
//! exactly why a gesture fired (or didn't) as you make it.
//!
//! Architecture:
//!   - `ipc_thread` owns the blocking socket reader on a background std
//!     thread and forwards typed `IpcMsg` values into an
//!     `async_channel::Sender`
//!   - `ui` builds the GTK4 widget tree and returns a closure that
//!     consumes `IpcMsg` and updates widgets
//!   - `main` wires them with `glib::spawn_future_local` — a GTK main
//!     loop task that awaits `receiver.recv()` and calls the UI closure
//!
//! `async_channel` is the replacement for the old `glib::MainContext::
//! channel` API that was removed in glib 0.20. The pattern is:
//!     (sender, receiver) = async_channel::unbounded();
//!     thread::spawn(|| sender.send_blocking(msg));
//!     glib::spawn_future_local(async move { while let Ok(msg) =
//!       receiver.recv().await { handle(msg); } });
//! which keeps the thread blocking-safe and the UI future-friendly.
//!
//! Requires niri built with `#[cfg(debug_assertions)]` — release builds
//! don't emit `RecognitionFrame` events, so this tool will connect but
//! show zero frame updates until a debug niri is running.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};

mod ipc_thread;
mod ui;

const APP_ID: &str = "dev.julian.niri-gesture-inspector";

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_window);
    app.run()
}

fn build_window(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("niri gesture inspector")
        .default_width(520)
        .default_height(420)
        .build();

    // Build the UI and get back a closure that accepts IpcMsg values.
    // The closure captures widget refs, so calling it from the main
    // thread mutates the live GTK tree.
    let handle_msg = ui::build(&window);

    // Async channel bridging the worker thread to the GTK main loop.
    // Unbounded because per-event payload is tiny and the IPC thread
    // must never block on a full queue — if the UI ever falls behind,
    // we'd rather accumulate a backlog than drop recognition frames.
    let (sender, receiver) = async_channel::unbounded::<ipc_thread::IpcMsg>();

    // Spawn the IPC reader. The closure captures `sender`, which is
    // Send + Clone. `send_blocking` is fine here because we're on a
    // dedicated std::thread, not an async task.
    ipc_thread::spawn(move |msg| {
        // send_blocking fails only if the receiver has been dropped,
        // which happens only at shutdown — ignore the error.
        let _ = sender.send_blocking(msg);
    });

    // Receiver future runs on the GTK main loop. `spawn_future_local`
    // ties it to the current (main) thread's main context so all UI
    // updates happen on the correct thread.
    glib::spawn_future_local(async move {
        while let Ok(msg) = receiver.recv().await {
            handle_msg(msg);
        }
    });

    window.present();
}
