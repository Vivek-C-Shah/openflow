# Security Policy

OpenFlow runs a global keyboard hook, captures microphone audio and pastes text into other apps, so we take security reports seriously.

## Reporting a vulnerability

Please **do not open a public issue**. Use GitHub's [private vulnerability reporting](https://github.com/Vivek-C-Shah/openflow/security/advisories/new) instead. We aim to acknowledge reports within 72 hours.

## What OpenFlow does and doesn't do

- The keyboard hook reacts only to the configured trigger key and Esc. It never logs, stores or sends other keystrokes.
- The microphone is open only between starting and ending a dictation.
- With default settings, audio and text never leave the machine. With a Groq key, audio and text go to Groq only for cloud or fallback processing.
- Local engines listen on `127.0.0.1` only, on random ports, and exist only around dictations. The control port (`127.0.0.1:47118`) accepts only `ping`, `reload`, `quit` and `demo`.
- Setup downloads engines from the official `ggml-org` GitHub releases and models from Hugging Face over HTTPS.

## Supported versions

Only the latest release receives security fixes.
