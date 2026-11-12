# OpenFlow

Free, local, private voice dictation for Windows, macOS and Linux.
**Double-tap Caps Lock, talk, tap it again.** Clean, punctuated text appears wherever your cursor is.

- **Uses nothing while idle.** The only thing running is a keyboard hook (~3 MB RAM, 0% CPU). The mic, speech model and AI model are not loaded.
- **Fast.** The AI engines start the moment you press the key, so they load while you talk. When you stop, the text appears in about a second on a GPU. They unload again 30 s after your last dictation.
- **Smart.** Removes "um"/"uh" and stutters, applies self-corrections ("at 3, no wait, 4" → "at 4"), adds punctuation, formats spoken lists, matches the app's tone (casual in WhatsApp/Slack, proper in email, literal in VS Code/terminals), uses your personal dictionary, and expands snippets. Handles English and Hinglish.
- **Hard to break.** AI output that drifts from what you said is rejected, and rule-based cleanup is used instead. If one engine fails, the other takes over. Silence never types "Thank you.". Your clipboard is restored after pasting. Every dictation is saved locally (`openflow last` re-copies it).
- **Free forever.** whisper.cpp + llama.cpp + open models, on your own machine. You can optionally add a free Groq key for a smarter cloud AI and an automatic backup.

## Install

**Windows** (PowerShell):
```powershell
irm https://raw.githubusercontent.com/Vivek-C-Shah/openflow/main/install.ps1 | iex
```

**macOS / Linux**:
```sh
curl -fsSL https://raw.githubusercontent.com/Vivek-C-Shah/openflow/main/install.sh | sh
```

Setup detects your GPU, downloads the right engines and models (one time, 1.3–3.5 GB), enables start-at-login and starts OpenFlow.

| Hardware | Speech model | AI model |
|---|---|---|
| NVIDIA GPU / Apple Silicon | Whisper large-v3-turbo (q5) | Qwen2.5 3B |
| CPU only | Whisper small (q5) | Qwen2.5 1.5B |

Flags: `openflow setup --cpu` / `--gpu` / `--cloud` (no downloads, needs a Groq key) / `--no-autostart`.

## Use

| Action | What happens |
|---|---|
| Double-tap trigger | Start hands-free dictation (you hear a rising beep) |
| Tap trigger | Finish: the text is pasted |
| Hold trigger | Push-to-talk: release to finish |
| Esc | Cancel |

Your Caps Lock state is left exactly as it was. A normal single press still toggles Caps Lock as usual.

```
openflow status | start | stop | restart
openflow config        # edit settings, then: openflow reload
openflow test 5        # record 5 s and show what would be typed
openflow last          # copy the last dictation again
openflow history       # what you said vs. what was typed
openflow doctor        # check the install
```

## Configure

`openflow config` opens a commented file. Highlights:

```toml
[trigger]
key = "CapsLock"            # or RightAlt, RightCtrl, F13...

[speech]
language = "auto"           # "en", "hi", ...
script = "latin"            # Hinglish in English letters; "native" keeps Devanagari

[engines]
idle_unload_seconds = 30    # 0 = free memory immediately after each dictation

[dictionary]
words = ["Kubernetes", "Supabase"]

[snippets]
"my email" = "me@example.com"

[apps]
"notion" = "casual"         # force a style for an app or window title
```

## How it works

```
idle:     openflow-daemon  ──  keyboard hook only (~3 MB, 0% CPU)
trigger:  mic on + whisper-server + llama-server start (they load while you talk)
stop:     16 kHz audio → silence check → Whisper → LLM cleanup → guard → snippets → paste
          → clipboard restored → saved to history
30 s idle: engines killed → all model memory freed
```

The engines are separate processes, so killing them really frees all their RAM and VRAM. On Windows they sit in a job object, so they can never outlive OpenFlow.

## Platform notes

- **macOS:** the default trigger is **Right Option**, because macOS doesn't report Caps Lock presses and releases reliably. Grant Accessibility and Microphone permission when asked. Engines are installed with Homebrew.
- **Linux:** needs X11 (Wayland global hotkeys are not supported yet). `sudo apt install libasound2 libxtst6`. Terminals get Ctrl+Shift+V automatically.

## Build from source

```sh
cargo build --release
./target/release/openflow setup
```

MIT licensed. Built on [whisper.cpp](https://github.com/ggml-org/whisper.cpp) and [llama.cpp](https://github.com/ggml-org/llama.cpp).
