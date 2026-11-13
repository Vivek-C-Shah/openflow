# Architecture

OpenFlow is built around two promises: it **costs nothing while idle**, and it **always produces text**.

## Processes

```
openflow-daemon (always running, ~2.4 MB)
├── main thread        keyboard hook + overlay window + tray (one Win32 message loop)
├── controller thread  trigger state machine, microphone, session lifecycle
├── control thread     127.0.0.1:47118 (ping / reload / quit / demo) + single-instance lock
├── engine reaper      sleeps on a condvar; kills engines after idle_unload_seconds
└── per-dictation thread: pipeline → paste → history

whisper-server  ┐ child processes that exist only around dictations,
llama-server    ┘ inside a kill-on-close job object (Windows) / PDEATHSIG (Linux)
```

Nothing polls. While idle, every thread is blocked on an OS wait: the hook's `GetMessage`, `recv()`, `accept()` or a condvar. The overlay only runs a 16 ms timer while it is animating.

## A dictation, step by step

1. **Trigger** (`trigger.rs`): key events go through a pure state machine. A double-tap starts hands-free mode, holding past `hold_ms` starts push-to-talk. Key presses are never swallowed. Instead, the machine counts toggle-key presses and emits `RestoreToggle` if the Caps Lock state ended up flipped.
2. **Start** (`daemon.rs`): the mic opens first (so no words are lost), then the start cue plays, the active app is captured for tone, and the engines are prewarmed. Each engine gets one throwaway warm-up request as soon as it's up, because the first inference after start is ~2 s slower (GPU kernel and buffer setup). The llama warm-up also fills the prompt cache with the system prompt and examples.
3. **Stop**: the mic closes and the audio goes to a worker thread:
   - resample to 16 kHz with a windowed-sinc filter; energy-based speech detection; trim silence; normalise quiet audio
   - audio is saved to history *before* processing, so a crash loses nothing
   - Whisper (beam search) → strip hallucinations such as "Thanks for watching" and bracketed noise tags
   - short, clean phrases skip the AI. Otherwise the LLM cleans the text with the app-specific style
   - **drift guard** (`text::accept`): reject output that is much longer, much shorter, or contains too many words that weren't said (fuzzy-matched, so spelling and misheard-word fixes still pass). On rejection, use rule-based cleanup
   - expand snippets, then paste via clipboard + Ctrl/⌘+V, restore the clipboard, and append to history
4. **Fallbacks**: each stage tries the preferred backend (local or Groq) and then the other. If every AI fails, the rules still produce text.

## Why separate engine processes?

Freeing a model inside a long-running process doesn't reliably return memory to the OS, and CUDA contexts hold VRAM until the process ends. Killing the process is the only way to guarantee a true zero footprint. As a bonus, we ship prebuilt official binaries and never compile C++.

## Platform layers

| Concern | Windows | macOS | Linux |
|---|---|---|---|
| Key listener | `WH_KEYBOARD_LL` (ignores injected keys) | `rdev` (CGEventTap) | `rdev` (X11 RECORD) |
| Paste | Ctrl+V via `enigo` | ⌘+V | Ctrl+V (Ctrl+Shift+V in terminals) |
| Overlay / tray | layered window + `Shell_NotifyIcon` | not yet | not yet |
| Engine cleanup | job object | pidfile | `PR_SET_PDEATHSIG` + pidfile |
| Engines installed from | ggml-org releases (CUDA/CPU) | Homebrew (Metal) | ggml-org releases (Vulkan/CPU) |
