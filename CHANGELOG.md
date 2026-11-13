# Changelog

All notable changes to this project are documented here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses [Semantic Versioning](https://semver.org/).

## [0.1.0] - 2026-11-13

### Added
- Caps Lock trigger: double-tap for hands-free, hold for push-to-talk, Esc to cancel. The Caps Lock state is restored after each dictation.
- On-demand whisper.cpp and llama.cpp engines that start and warm up while you talk and unload after 30 s idle.
- AI cleanup: fillers, self-corrections, misheard-word repair, punctuation, lists, and app-aware tone (casual, email, code). Supports English and Hinglish.
- Drift guard, rule-based fallback, local/cloud (Groq) cross-fallback, and silence and hallucination filtering.
- Overlay pill (idle bar, live voice bars, hands-free buttons, writing and error states) and tray icon with menu (Windows).
- Personal dictionary, snippets, per-app style overrides, and local history with `openflow last`.
- One-command setup with GPU detection, resumable downloads and start-at-login.
- CLI: `status`, `start`, `stop`, `reload`, `config`, `test`, `transcribe`, `history`, `doctor`.
