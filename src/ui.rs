//! GTK4 UI for the gesture inspector.
//!
//! Layout (top to bottom):
//!   - status bar (connection state, finger count, current `closest`)
//!   - four `ThresholdBar` widgets: swipe / spread / rotation / arc
//!   - classification flags row (`is_rotate`, `is_pinch`)
//!   - recent-lock history label
//!
//! Each `ThresholdBar` is a custom DrawingArea that renders a horizontal
//! fill bar with a threshold marker, color-coded by "distance from
//! threshold crossing". Simpler than hand-wiring gtk::ProgressBar for
//! every stat and gives us finer control over threshold ticks, overshoot,
//! and color transitions.

use gtk4::cairo::Context;
use gtk4::prelude::*;
use gtk4::{
    Align, ApplicationWindow, DrawingArea, Frame, Label, Orientation,
};
use std::cell::RefCell;
use std::f64::consts::PI;
use std::rc::Rc;

use crate::ipc_thread::{FrameSnapshot, IpcMsg};

/// Rolling UI state updated from every `IpcMsg` received on the main thread.
#[derive(Default)]
pub struct UiState {
    /// Most recent recognition frame, or None if no touch is active.
    pub latest: Option<FrameSnapshot>,
    /// Most recent connection status string ("connected" / error text).
    pub status: String,
    /// Last-latched gesture info ("RotateCcw fingers=5" etc.), shown in
    /// the history pane so users can see which classification won.
    pub last_lock: Option<String>,
    /// Last gesture end info.
    pub last_end: Option<String>,
}

/// One bar + label widget for a single threshold metric.
///
/// Draws horizontally:
///   [   filled region    |                threshold tick            ]
///   0                    current                                 max
/// with color:
///   - neutral grey when current < 0.5 × threshold
///   - yellow when approaching threshold
///   - green when `is_rotate`/`is_pinch` is true (gesture locked in)
///   - red when threshold crossed but gesture not committed (race state)
pub struct ThresholdBar {
    pub container: gtk4::Box,
    value_label: Label,
    drawing: DrawingArea,
    /// Shared state cell — updated from the main thread before each
    /// queue_draw(). Using Rc<RefCell<>> because the draw callback is a
    /// closure that outlives the setter.
    state: Rc<RefCell<BarState>>,
    /// Unit suffix shown in the value label ("px", "°", etc.).
    unit: String,
    /// Multiplier applied to native `current` / `threshold` before they're
    /// rendered in the label. Lets the rotation bar store radians
    /// internally (so bar proportions match) while displaying degrees.
    display_scale: f64,
    /// Layout mode: unidirectional fills 0→right, bidirectional places
    /// zero in the middle with a fill growing left (negative) or right
    /// (positive).
    bidirectional: bool,
}

#[derive(Default, Clone)]
struct BarState {
    /// Signed current value (may be negative in bidirectional mode).
    current: f64,
    /// Positive threshold. In bidirectional mode the symmetric
    /// negative threshold `-threshold` is also drawn.
    threshold: f64,
    /// Classifier flag for this bar is set — forces green fill to mean
    /// "this trigger has passed its commit gates".
    active: bool,
    /// True once we've received at least one frame. Before that we show
    /// an em-dash instead of a meaningless "0.0 / 0.0" label.
    seen: bool,
    /// Mirrors `ThresholdBar::bidirectional` so the draw closure can see
    /// it without holding a reference to the parent.
    bidirectional: bool,
}

impl ThresholdBar {
    pub fn new(title: &str, unit: &str, display_scale: f64) -> Self {
        Self::build(title, unit, display_scale, false)
    }

    /// Bidirectional bar: zero in the center, symmetric ± thresholds.
    /// Use for signed values like `spread_change` (pinch in/out) and
    /// `rotation_rad` (ccw/cw).
    pub fn new_bidirectional(title: &str, unit: &str, display_scale: f64) -> Self {
        Self::build(title, unit, display_scale, true)
    }

    fn build(title: &str, unit: &str, display_scale: f64, bidirectional: bool) -> Self {
        let container = gtk4::Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(2)
            .margin_top(4)
            .margin_bottom(4)
            .build();

        let header = gtk4::Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(8)
            .build();
        let title_label = Label::builder()
            .label(title)
            .xalign(0.0)
            .width_chars(10)
            .build();
        let value_label = Label::builder()
            .label("–")
            .xalign(1.0)
            .hexpand(true)
            .build();
        header.append(&title_label);
        header.append(&value_label);
        container.append(&header);

        let drawing = DrawingArea::builder()
            .content_height(18)
            .hexpand(true)
            .build();
        container.append(&drawing);

        let state = Rc::new(RefCell::new(BarState {
            bidirectional,
            ..BarState::default()
        }));
        {
            let state = state.clone();
            drawing.set_draw_func(move |_, cr, width, height| {
                let s = state.borrow();
                draw_bar(cr, width as f64, height as f64, &s);
            });
        }

        Self {
            container,
            value_label,
            drawing,
            state,
            unit: unit.to_string(),
            display_scale,
            bidirectional,
        }
    }

    /// Push a new frame's worth of data into the bar. `current` and
    /// `threshold` are in native units (px or radians); the bar converts
    /// to display units via `display_scale` before labelling.
    pub fn update(&self, current: f64, threshold: f64, active: bool) {
        {
            let mut s = self.state.borrow_mut();
            s.current = current;
            s.threshold = threshold.abs();
            s.active = active;
            s.seen = true;
        }
        self.value_label
            .set_text(&self.format_label(current, threshold.abs()));
        self.drawing.queue_draw();
    }

    /// Reset the fill to zero but keep the threshold tick and label so
    /// users can see the target even while no touch is active. Called on
    /// `GestureEnd`.
    pub fn clear(&self) {
        let (threshold, seen) = {
            let mut s = self.state.borrow_mut();
            s.current = 0.0;
            s.active = false;
            (s.threshold, s.seen)
        };
        if seen {
            self.value_label
                .set_text(&self.format_label(0.0, threshold));
        } else {
            self.value_label.set_text("–");
        }
        self.drawing.queue_draw();
    }

    fn format_label(&self, current: f64, threshold: f64) -> String {
        let cur_disp = current * self.display_scale;
        let thr_disp = threshold * self.display_scale;
        if self.bidirectional {
            format!("{:+.1} / ±{:.1} {}", cur_disp, thr_disp, self.unit)
        } else {
            format!("{:.1} / {:.1} {}", cur_disp, thr_disp, self.unit)
        }
    }
}

fn draw_bar(cr: &Context, width: f64, height: f64, state: &BarState) {
    // Track background — same for both modes.
    cr.set_source_rgb(0.12, 0.12, 0.14);
    cr.rectangle(0.0, 0.0, width, height);
    cr.fill().ok();

    // Overshoot headroom: show up to 1.5× threshold so users can see how
    // far past a crossing they went.
    let display_max = (state.threshold * 1.5).max(1e-6);
    let magnitude = state.current.abs();

    // Shared color ramp — uses magnitude so the bidirectional case still
    // turns amber/red once either direction crosses its threshold.
    let (r, g, b) = if state.active {
        (0.20, 0.85, 0.35)
    } else if magnitude >= state.threshold {
        (0.90, 0.30, 0.30)
    } else if magnitude >= state.threshold * 0.5 {
        (0.95, 0.75, 0.15)
    } else {
        (0.35, 0.55, 0.85)
    };

    if state.bidirectional {
        let center = width / 2.0;
        let frac = (magnitude / display_max).clamp(0.0, 1.0);
        let half_width = (width / 2.0) * frac;

        // Fill grows from center toward the sign of `current`.
        cr.set_source_rgb(r, g, b);
        if state.current >= 0.0 {
            cr.rectangle(center, 0.0, half_width, height);
        } else {
            cr.rectangle(center - half_width, 0.0, half_width, height);
        }
        cr.fill().ok();

        // Zero line — slightly brighter than the background, thinner
        // than the threshold ticks so it reads as an axis.
        cr.set_source_rgb(0.55, 0.55, 0.60);
        cr.set_line_width(1.0);
        cr.move_to(center.round() + 0.5, 0.0);
        cr.line_to(center.round() + 0.5, height);
        cr.stroke().ok();

        // Symmetric threshold ticks at ±threshold.
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(1.5);
        let tick_frac = (state.threshold / display_max).clamp(0.0, 1.0);
        let offset = (width / 2.0) * tick_frac;
        for x_raw in [center + offset, center - offset] {
            let x = x_raw.round() + 0.5;
            cr.move_to(x, 0.0);
            cr.line_to(x, height);
            cr.stroke().ok();
        }
    } else {
        let frac = (magnitude / display_max).clamp(0.0, 1.0);
        cr.set_source_rgb(r, g, b);
        cr.rectangle(0.0, 0.0, width * frac, height);
        cr.fill().ok();

        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(1.5);
        let threshold_frac = (state.threshold / display_max).clamp(0.0, 1.0);
        let x = (width * threshold_frac).round() + 0.5;
        cr.move_to(x, 0.0);
        cr.line_to(x, height);
        cr.stroke().ok();
    }
}

/// Build the full inspector window UI. Returns a closure that the main
/// thread calls for every `IpcMsg` received from the background reader.
///
/// `use<>` narrows the return type's lifetime capture under Rust 2024
/// edition rules — the returned closure doesn't borrow `window`
/// (everything it needs is cloned GTK handles), so we explicitly opt
/// out of capturing the `&ApplicationWindow` lifetime.
pub fn build(window: &ApplicationWindow) -> impl Fn(IpcMsg) + use<> {
    let root = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();

    // Status header.
    let status_label = Label::builder()
        .label("Waiting for niri connection...")
        .xalign(0.0)
        .css_classes(vec!["title-4".to_string()])
        .build();
    root.append(&status_label);

    let fingers_row = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .build();
    let fingers_label = Label::builder()
        .label("fingers: –")
        .xalign(0.0)
        .css_classes(vec!["title-3".to_string()])
        .build();
    let closest_label = Label::builder()
        .label("closest: –")
        .xalign(0.0)
        .css_classes(vec!["title-3".to_string()])
        .build();
    fingers_row.append(&fingers_label);
    fingers_row.append(&closest_label);
    root.append(&fingers_row);

    // Four threshold bars.
    //
    // - swipe / arc  are magnitudes → unidirectional.
    // - pinch / rotation are signed → bidirectional (pinch-in vs
    //   pinch-out, ccw vs cw).
    //
    // Rotation stores radians internally (matching niri's native
    // representation so bar proportions are correct) and displays
    // degrees for legibility.
    let swipe_bar = ThresholdBar::new("swipe", "px", 1.0);
    let pinch_bar = ThresholdBar::new_bidirectional("pinch", "px", 1.0);
    let rot_bar = ThresholdBar::new_bidirectional("rotation", "°", 180.0 / PI);
    let arc_bar = ThresholdBar::new("arc", "px", 1.0);

    let bars_box = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(4)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    bars_box.append(&swipe_bar.container);
    bars_box.append(&pinch_bar.container);
    bars_box.append(&rot_bar.container);
    bars_box.append(&arc_bar.container);
    root.append(&bars_box);

    // Classifier flags row with an inline legend. The row shows the
    // per-frame commit flags; the legend below clarifies what "committed"
    // means versus the header's `closest` field (which is just the
    // current progress leader, not the race winner).
    let flags_row = gtk4::Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(16)
        .halign(Align::Start)
        .margin_top(6)
        .build();
    let rotate_flag = Label::builder().label("is_rotate: –").xalign(0.0).build();
    let pinch_flag = Label::builder().label("is_pinch:  –").xalign(0.0).build();
    flags_row.append(&rotate_flag);
    flags_row.append(&pinch_flag);
    root.append(&flags_row);

    let legend = Label::builder()
        .label(
            "is_rotate ✓  rotation passed threshold AND dominates swipe+spread\n\
             is_pinch  ✓  spread passed threshold AND dominates swipe\n\
             closest       current leader by % of threshold (not the committed winner)",
        )
        .xalign(0.0)
        .wrap(true)
        .css_classes(vec!["dim-label".to_string()])
        .build();
    root.append(&legend);

    // History pane — last lock + last end.
    let history_frame = Frame::builder().label("Recent").build();
    let history_box = gtk4::Box::builder()
        .orientation(Orientation::Vertical)
        .spacing(2)
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    let last_lock_label = Label::builder()
        .label("last lock: –")
        .xalign(0.0)
        .css_classes(vec!["title-3".to_string()])
        .build();
    let last_end_label = Label::builder().label("last end:  –").xalign(0.0).build();
    history_box.append(&last_lock_label);
    history_box.append(&last_end_label);
    history_frame.set_child(Some(&history_box));
    root.append(&history_frame);

    window.set_child(Some(&root));

    // State cell shared with the closure returned to the caller.
    let state = Rc::new(RefCell::new(UiState::default()));

    // Capture widgets by clone into the closure. GTK widgets are
    // reference-counted GObjects so cloning is cheap and shares
    // underlying state.
    let state_c = state.clone();
    move |msg: IpcMsg| {
        let mut s = state_c.borrow_mut();
        match msg {
            IpcMsg::Connected => {
                s.status = "connected".into();
                status_label.set_text("Connected to niri — touch the screen");
            }
            IpcMsg::Frame(frame) => {
                // swipe has no per-frame classifier bool; its "commit"
                // only happens at end-of-gesture when the race resolves.
                swipe_bar.update(
                    frame.swipe_distance,
                    frame.swipe_trigger_distance,
                    false,
                );
                // pinch + rotation are signed — the bidirectional bars
                // use the sign to show direction (pinch-in/out, ccw/cw).
                pinch_bar.update(
                    frame.spread_change,
                    frame.pinch_trigger_distance,
                    frame.is_pinch,
                );
                rot_bar.update(
                    frame.rotation_rad,
                    frame.rotation_trigger_angle_rad,
                    frame.is_rotate,
                );
                arc_bar.update(
                    frame.rotation_arc,
                    frame.rotation_arc_trigger_distance,
                    frame.is_rotate,
                );

                fingers_label.set_text(&format!("fingers: {}", frame.finger_count));
                closest_label.set_text(&format!("closest: {}", frame.closest));
                rotate_flag.set_text(&format!(
                    "is_rotate: {}",
                    if frame.is_rotate { "✓" } else { "✗" }
                ));
                pinch_flag.set_text(&format!(
                    "is_pinch:  {}",
                    if frame.is_pinch { "✓" } else { "✗" }
                ));

                s.latest = Some(frame);
            }
            IpcMsg::GestureLocked {
                tag,
                trigger,
                finger_count,
            } => {
                let text = format!(
                    "last lock:\n  trigger: {trigger}\n  fingers: {finger_count}\n  tag: {tag}"
                );
                last_lock_label.set_text(&text);
                s.last_lock = Some(text);
            }
            IpcMsg::GestureEnded { tag, completed } => {
                let status = if completed { "completed" } else { "cancelled" };
                let text = format!("last end:  tag={tag} ({status})");
                last_end_label.set_text(&text);
                s.last_end = Some(text);

                // Touch ended — reset fills to zero but keep the
                // threshold ticks + labels visible so users can see
                // their targets between gestures.
                swipe_bar.clear();
                pinch_bar.clear();
                rot_bar.clear();
                arc_bar.clear();
                fingers_label.set_text("fingers: –");
                closest_label.set_text("closest: –");
                rotate_flag.set_text("is_rotate: ✗");
                pinch_flag.set_text("is_pinch:  ✗");
            }
            IpcMsg::Disconnected(reason) => {
                status_label.set_text(&format!("Disconnected: {reason}"));
                s.status = format!("disconnected: {reason}");
            }
        }
    }
}
