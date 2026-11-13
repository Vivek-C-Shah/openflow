# Contributing to OpenFlow

Thanks for helping. The most valuable contributions right now are **testing on macOS and Linux**, accuracy reports from real dictations, and fixes for issues labelled `good first issue`.

## Development setup

```sh
git clone https://github.com/Vivek-C-Shah/openflow && cd openflow
cargo build
cargo test
./target/debug/openflow setup --no-autostart   # downloads engines + models once
./target/debug/openflow run                    # daemon in the foreground with live logs
```

Linux needs `libasound2-dev libx11-dev libxtst-dev libxi-dev libxcb1-dev libxdo-dev libxkbcommon-dev pkg-config`.

Commands that help during development:

| Command | Use |
|---|---|
| `openflow run` | Run the daemon in the foreground with logs on stderr |
| `openflow transcribe a.wav b.wav` | Run WAV files through the full pipeline (the first is cold, the rest warm) |
| `openflow transcribe --talk 3000 a.wav` | Wait 3 s first, like a user speaking while engines warm up |
| `openflow test 5` | Record 5 s from the mic and print the result |
| `openflow demo` | Animate the running daemon's overlay through every state |
| `openflow render-preview docs/img` | Render the overlay states to PNG |
| `OPENFLOW_DEBUG=1` | Verbose logs |
| `OPENFLOW_HOME`, `OPENFLOW_CONFIG` | Use a separate data folder or config file |

## Project layout

| Path | What it does |
|---|---|
| `src/trigger.rs` | Pure state machine for double-tap, hold and Esc (fully unit-tested) |
| `src/keyboard.rs` | Global key listener: native hook on Windows, `rdev` elsewhere |
| `src/daemon.rs` | Controller: turns trigger actions into recording, processing and pasting |
| `src/engines.rs` | Starts and kills whisper-server / llama-server on demand |
| `src/stt.rs`, `src/intelligence.rs` | Speech-to-text and AI cleanup (local + Groq with fallback) |
| `src/text.rs` | Rule-based cleanup, the drift guard, hallucination filter, snippets |
| `src/ui/` | Overlay renderer (cross-platform) and Windows window + tray |
| `src/setup.rs` | Installer: hardware detection, downloads, autostart |

The design principles are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Please keep them intact: **nothing heavy runs while idle**, and **text is always produced, even if every AI backend fails**.

## Pull requests

1. Open an issue first for anything bigger than a small fix, so we can agree on the approach.
2. Keep PRs focused. Add or update tests for logic changes. `trigger.rs` and `text.rs` are easy to test.
3. Run `cargo fmt`, `cargo clippy` and `cargo test` before pushing. CI runs on Windows, macOS and Linux.
4. Describe how you tested it, especially for platform-specific code.

## Reporting dictation accuracy

Run `openflow history` to see what was heard and what was typed. Include the `said:` / `typed:` lines, the language you spoke, your microphone, and your `[speech]` settings. Attach audio from `history/audio/` only if you're comfortable sharing it.
