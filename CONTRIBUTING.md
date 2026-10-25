# Contributing

Thanks for helping. Issues and pull requests are welcome.

## Setup

```bash
cargo run               # the window (starts a server in the background)
cargo run -- serve      # only the server
cargo test              # unit, API, PTY and two-server tests
```

On Linux the window needs the usual X11/Wayland libraries (on Debian/Ubuntu:
`libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev`).
`cargo build --no-default-features` builds the server alone.

## Before a pull request

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass (CI runs them
  on Linux and macOS).
- New logic in `src/core` (pure, no I/O) has unit tests; behaviour across processes or the
  network has a test in `tests/`.
- Features land in the HTTP API first, then in the window, the web client and the SDK.
- Keep it light: kimo should stay one small binary with a small memory footprint.

## Layout

```
src/main.rs        entry point: kimo (window), serve, pair, agents, send…
src/core/          pure logic and parsing (models, access rules, launch, usage, activity…)
src/services/      sessions (PTYs), projects, accounts, usage, peers, skills, talk, stats, network
src/api/           HTTP routes, websockets, access control, embedded web client
src/desktop/       the window (egui) and its terminal (alacritty_terminal)
src/cli.rs         command-line client for people and agents
web/               phone/web client, SDK (web/sdk), xterm.js
skills/            the bundled kimo-agents skill
tests/             integration tests (fake agents on PATH, two paired servers)
```

By contributing you agree that your contributions are licensed under MIT OR Apache-2.0.
