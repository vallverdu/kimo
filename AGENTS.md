# kimo: instructions for development agents

Read [the product direction](docs/product-direction.md) before choosing or implementing work.
It records the owner's intent and the acceptance scenarios for the product. Use the
priority section at the top of [TODO.md](TODO.md) to choose work; the order of the later
feature inventories is not a priority ranking.

## Product principles

- kimo is a **mobile-first agent development environment**. The primary journey is a phone
  or tablet PWA connecting to a computer, creating a project from nothing on that computer,
  starting and directing agents there, and having them deliver or publish what the user asks.
- The server owns projects, processes and resources. It must run headless on supported
  macOS and Linux machines, including laptops, Mac minis and capable GPU PCs. The native
  window is a client; core workflows must be usable from the PWA without opening it.
- Paired kimos let a user continue work on its original host from another device or kimo.
  Keep the selected host clear. A project on a NUC stays on the NUC when accessed from a Mac.
- Keep the Rust core small and efficient with several agents and terminals. Measure kimo's
  overhead separately from its child processes. Show resource pressure and useful recovery
  actions on mobile. Pause saves CPU; stopping a resumable agent frees its RAM.
- Switching between already authenticated Claude Code or Codex accounts must preserve each
  conversation, without logout/login or manual chat reconstruction. Track remaining work on
  limit detection and phone-friendly account setup honestly.
- kimo is a foundation for custom PWAs and agent orchestration. Implement capabilities in
  the shared HTTP API and SDK, then the PWA and native client. Preserve scoped app access.
- Keep projects as ordinary folders. Store kimo's internal state outside them; do not add a
  mandatory file browser, editor, proprietary project format or heavy orchestration framework
  to the core. Let agents and custom apps supply richer workflows.
- kimo is the node agent on every machine. Coordination across machines (which machine runs
  a job, waking and switching machines off, workflows, artifacts) lives in `kimo-fleet`
  (`fleet/`), a separate binary on one always-on machine that reaches kimos only through
  their API with one-way fleet keys. See [docs/fleet.md](docs/fleet.md).
- Close gaps in the phone-to-server journey before optional desktop polish or copying
  competitor features. A checked-off backend task alone does not prove a usable phone flow.

## Architecture

Layers: entry point → UI (native window, web client) → services (I/O) → core (pure logic).
Dependencies point down. The window, CLI and PWAs are clients of the same API.

```
src/main.rs        entry point: kimo / serve / pair / agents / send / read / wait / new / peer…
src/cli.rs         command-line client for people and agents, over src/client.rs
src/desktop/       egui window and alacritty_terminal widget; an API client
src/api/           HTTP, websockets, access control, host/id forwarding, embedded web client
src/services/      I/O: sessions, projects, files/git, accounts, usage, talk, peers, devices,
                   jobs, node facts, power, awake, ports, skills,
                   store (the only code writing state.json), stats, network
src/core/          pure synchronous logic, models and parsing; no I/O
src/config.rs      clap flags with env fallbacks (see .env.example)
web/               phone/web client, CSS, JS modules and SDK (web/sdk)
skills/            bundled kimo-agents skill, embedded in the binary
fleet/             kimo-fleet coordinator crate: core (registry, power states, scheduling,
                   jobs/workflows), services (SQLite, keys, network, the loop), api, CLI
```

## Implementation rules

- Pure logic goes in `core/` with unit tests next to it; process behaviour is covered in
  `tests/sessions.rs`, API behaviour in `tests/api.rs`, paired hosts in `tests/peers.rs`,
  and the coordinator with real kimos in `fleet/tests/fleet.rs`. Tests and dev servers never
  switch the machine off: use `KIMO_SHUTDOWN_COMMAND` or `set_shutdown_command`.
- Keep it light: no new heavy dependencies, no per-session async tasks beyond the two PTY threads.
- State is plain JSON in `~/.kimo/state.json` (or `KIMO_DATA_DIR`); preserve unknown fields
  with `#[serde(flatten)] extra`.
- Owner = the machine itself (not through a proxy), paired devices, or `KIMO_TOKEN`.
  A fleet coordinator's key (pairing role `fleet`) reaches only `/status`, `/node`, `/jobs`
  and `/power`. Keep these rules in `api::auth` + `core/access.rs`, tested in `tests/api.rs`.
- Projects are never deleted from disk by kimo: "forget" only unregisters them.
- Credentials never go in state or git: they live in each account's config home.
- Every API change goes through the SDK too. kimo's own PWA uses only `web/sdk/`, so other
  apps get the same features subject to their scopes and ownership requirements.
- Errors reach the user as `{ "error": "plain words" }` and a toast; details go to the log.
- The web client uses `web/css/base.css`, `app.css`, `kimo.css` and `tokens.css`: use the
  design tokens, including `--color-*`, instead of hard-coded colours or font sizes.
- Icons are inline SVG line icons. The brand master is `assets/brand/kimo.svg`;
  `python3 scripts/make-icons.py` generates the icons.
- For code changes, run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D
  warnings` and `cargo test --workspace` before committing; changes to shared/server code
  must also preserve the `cargo build --no-default-features` headless build.
- New features go in `CHANGELOG.md` under *Unreleased* (a release turns it into the GitHub
  release notes); `README.md`, `docs/api.md` and `docs/fleet.md` describe what they do and
  their defaults. Releases happen only when the owner asks.
- Keep `TODO.md` current. Describe what is implemented, what was actually verified and what
  remains. For affected product scenarios, verify the PWA and host routing as well as the
  API, and report unavailable real-device or provider checks explicitly.
