# kimo: conventions for agents

Layers: entry point → UI (window, web client) → services (I/O) → core (pure logic). The web client has its
own small CSS (`web/css/base.css`, `app.css`, `kimo.css`) on top of `css/tokens.css`: never
hard-code colours or font sizes, use the `--color-*` variables. Icons are inline SVG line icons.
Brand: `assets/brand/kimo.svg` (master) → `python3 scripts/make-icons.py` makes every icon.

## Layers (dependencies point down)

```
src/main.rs        entry point only: kimo (window) / serve / pair / agents / send / read / wait / new / peer…
src/cli.rs         the command-line client (people and agents), over src/client.rs
src/desktop/       the window (egui) and its terminal widget (alacritty_terminal); a client of the API
src/api/           HTTP routes, websockets, access control, forwarding of host/id calls, embedded web client
src/services/      I/O: sessions (PTYs, lifecycle, update handoff), projects, files and git, accounts,
                   usage, talk (messages, replies), peers (other kimos), devices (pairing, fleet keys),
                   jobs (detached `kimo job-run`), node (machine facts), power (guarded shutdown),
                   awake, ports, skills, store (the only code writing state.json), stats, network
src/core/          pure, synchronous, no I/O: models, access, launch, lifecycle, activity, usage, replies,
                   jobs, node facts parsing, power blockers, procfs/ps parsing, network parsing, git,
                   skills front matter, host names, sha256
src/config.rs      clap flags with env fallbacks (see .env.example)
web/               phone/web client (base.css, app.css, kimo.css; js/ modules) and the SDK (web/sdk)
skills/            the bundled kimo-agents skill (embedded in the binary)
fleet/             kimo-fleet, the coordinator (its own crate, same layers: core pure, services I/O,
                   api): registry, power states, scheduling, jobs and workflows, SQLite, artifacts
```

## Rules

- Pure logic goes in `core/` with unit tests next to it; process behaviour is covered in `tests/sessions.rs`.
- Keep it light: no new heavy dependencies, no per-session async tasks beyond the two PTY threads.
- State is plain JSON in `~/.kimo/state.json`, unknown fields preserved (`#[serde(flatten)] extra`).
- Owner = the machine itself (not through a proxy, addressed by its own names), paired devices, or KIMO_TOKEN. A fleet coordinator's key (pairing role `fleet`) reaches only `/status`, `/node`, `/jobs`, `/power`. Keep those rules in `api::auth` + `core/access.rs`, tested in `tests/api.rs`.
- kimo is the node agent; anything across machines (scheduling, power of other machines, workflows) goes in `fleet/`, which talks to kimos only through their HTTP API. Fleet behaviour is tested in `fleet/tests/fleet.rs` with real kimos on local ports.
- Never let a test or a dev server switch the machine off: set `KIMO_SHUTDOWN_COMMAND` (or `set_shutdown_command`) to a stand-in.
- Projects are never deleted from disk by kimo: "forget" only unregisters them.
- Credentials never go in state or git: they live in each account's config home.
- The native window and every PWA use the same HTTP API: new features land in the API first, then in the window, the web client and the SDK.
- Every API change goes through the SDK too: kimo's own PWA uses only `web/sdk/`, so other apps get the same features.
- Errors reach the user as `{ "error": "plain words" }` and a toast; details go to the log.
- Run `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` before committing, and keep `cargo build --no-default-features` working.
- New features go in CHANGELOG.md under *Unreleased*; README.md and docs (api.md, fleet.md) say what they do and what they do by default.
- TODO.md is the task list: tick what you finish, add follow-ups you discover.
