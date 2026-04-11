# niri-gesture-inspector

A GTK4 live scope for the niri touch gesture classifier. Subscribes to the
debug `RecognitionFrame` IPC stream and renders swipe / pinch / rotation /
arc threshold bars in real time, alongside the `is_rotate` / `is_pinch`
classification flags and a recent-lock history pane.

Intended for tuning the touchscreen gesture knobs
(`swipe-trigger-distance`, `pinch-dominance-ratio`,
`rotation-trigger-angle`, etc.) and understanding why a given touch
sequence committed to swipe vs. pinch vs. rotate.

## Requirements: debug build of niri

The `Event::RecognitionFrame` IPC variant is gated behind
`#[cfg(debug_assertions)]` in the compositor — **release builds of niri
never emit recognition frames**, so the inspector will connect but show
nothing. You need a debug-built niri running as the compositor.

Each niri instance binds its own IPC socket at
`$XDG_RUNTIME_DIR/niri.<wayland-display>.<pid>.sock` and exports that
path via `$NIRI_SOCKET`. A debug niri run as a nested session (or as a
separate TTY) will therefore have a different `NIRI_SOCKET` than any
system-installed release niri, and you must point the inspector at the
debug one.

```bash
# Start a debug niri (e.g. nested, from the niri checkout)
cargo run -- --session   # or however you launch your debug compositor

# In that session's environment, $NIRI_SOCKET is already set correctly.
# From a separate shell, inherit it explicitly if needed:
export NIRI_SOCKET=/run/user/1000/niri.wayland-1.123456.sock
niri-gesture-inspector
```

If you forget and run the inspector against a release niri, the UI shows
"connected" but no bars ever move — that's the symptom of the debug gate
stripping the event.

## Build / install

```bash
./install_niri_gesture_inspector.sh
```

Builds in debug mode, kills any running instance, and installs to
`/usr/local/bin/niri-gesture-inspector`.

## UI

- **fingers / closest** — active finger count and the current
  progress-leader (not the committed winner).
- **swipe / pinch / rotation / arc** — threshold bars. Unidirectional for
  magnitudes (swipe, arc); bidirectional for signed values (pinch in/out,
  ccw/cw rotation).
- **is_rotate / is_pinch** — per-frame commit flags (rotate requires
  passing dominance over both swipe and spread; pinch requires passing
  dominance over swipe).
- **Recent** — last lock (trigger / fingers / tag) and last end.
