# Contributing

Thanks for helping. Issues and pull requests are welcome.

Read [the product direction](docs/product-direction.md) and [AGENTS.md](AGENTS.md) first.
kimo's primary workflow is creating and directing projects from a phone PWA on a remote
computer, and continuing work through paired kimos. Prioritise that workflow and the shared
API/SDK when choosing changes; the feature inventories in `TODO.md` are not a priority order.

## Setup

```bash
cargo run               # the window (starts a server in the background)
cargo run -- serve      # only the server
cargo test --workspace  # unit, API, PTY, two-server and fleet tests
cargo run -p kimo-fleet # the fleet coordinator
```

On Linux the window needs the usual X11/Wayland libraries (on Debian/Ubuntu:
`libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev`).
`cargo build --no-default-features` builds the server alone.

## Before a pull request

- `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` pass (CI runs them on Linux and macOS).
- New features are in `CHANGELOG.md` under *Unreleased*, and in the README and docs.
- New logic in `src/core` (pure, no I/O) has unit tests; behaviour across processes or the
  network has a test in `tests/`.
- Features land in the HTTP API and SDK, then in the phone/web and native clients. Verify
  affected mobile and paired-host workflows; record real-device checks separately from tests.
- Keep it light: kimo should stay one small binary with a small memory footprint.
- Changes to shared/server code must preserve `cargo build --no-default-features`.

## Layout

```
src/main.rs        entry point: kimo (window), serve, pair, agents, send…
src/core/          pure logic and parsing (models, access rules, launch, usage, activity…)
src/services/      sessions (PTYs), projects, files/git, accounts, usage, peers, devices, jobs,
                   node facts, power, skills, talk, stats, network
src/api/           HTTP routes, websockets, access control, embedded web client
src/desktop/       the window (egui) and its terminal (alacritty_terminal)
src/cli.rs         command-line client for people and agents
web/               phone/web client, SDK (web/sdk), xterm.js
skills/            the bundled kimo-agents skill
tests/             integration tests (fake agents on PATH, two paired servers, jobs)
fleet/             kimo-fleet: the coordinator (registry, power, scheduling, workflows) and
                   its tests with real kimos on local ports (fleet/tests)
```

Tests never switch the machine off: kimos in tests use a stand-in shutdown command.

By contributing you agree that your contributions are licensed under MIT OR Apache-2.0.
