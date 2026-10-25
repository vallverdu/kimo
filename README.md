# kimo

A small, mobile-first **agent development environment** written in Rust. One 3.5 MB binary runs
your Claude Code and Codex agents (and plain shells) in terminals on your machine and serves a
PWA to drive them from your phone over Tailscale, ZeroTier or your LAN.

kimo is meant to be the **agent backend for all your PWAs**: Everpano, a project manager or any
other app can watch and drive the same agents with its own key, through a small JS SDK and an
embeddable `<kimo-terminal>`. kimo's own interface is just the first of those apps.

It exists because attention is the bottleneck: you should be able to follow, pause, move and
continue dozens of agents from a phone, without logging out and in when a subscription runs
out, and without 26 idle agents eating 30 GB of RAM.

## Screenshots

Real run on a laptop: a Claude Code agent (≈650 MB, the reason stopping matters), two shells,
one paused, one stopped; kimo itself uses 6 MB.

### On the phone

| Agents | Every session | Claude Code in a terminal |
|---|---|---|
| ![Agents overview](docs/screenshots/phone-agents.png) | ![All sessions: running, paused, stopped](docs/screenshots/phone-agents-all.png) | ![Claude Code in the phone terminal](docs/screenshots/phone-terminal.png) |

| Paused session | Accounts | New session |
|---|---|---|
| ![A paused session: frozen, no CPU](docs/screenshots/phone-paused.png) | ![Accounts and moving sessions](docs/screenshots/phone-accounts.png) | ![New session sheet](docs/screenshots/phone-new-session.png) |

A session is *paused* (frozen with SIGSTOP, no CPU, memory kept) or *stopped* (memory freed,
**continue** brings the conversation back). The terminal has a key bar and a compose box so an
agent can be driven without a keyboard.

### On the desktop

![Agents on desktop](docs/screenshots/desktop-agents.png)

![A terminal on desktop: the test suite running inside kimo](docs/screenshots/desktop-terminal.png)

![Accounts on desktop: move every session of a login to another subscription](docs/screenshots/desktop-accounts.png)

## What it does

- **Terminals**: create Claude, Codex or shell sessions in any folder; open them full screen
  with a phone key bar (esc, tab, ^C, arrows) and a compose box for typing messages.
- **Pause / stop / continue**: *pause* freezes the whole process tree (SIGSTOP: no CPU, memory
  kept); *stop* ends it to free the memory; *continue* brings the conversation back. Each Claude
  session owns its conversation id (`--session-id`, then `--resume <id>`), so agents sharing a
  folder never pick up each other's conversation; Codex uses `codex resume --last`.
- **Accounts without logout**: each account gets its own config home
  (`~/.kimo/accounts/<id>`, via `CLAUDE_CONFIG_DIR` / `CODEX_HOME`); sign in once per account.
  Every account home links the default home's conversations and settings (`projects/`,
  `settings.json`, `skills/`… for Claude; `sessions/`, `config.toml`… for Codex), so moving a
  session to another account restarts it under the new subscription *and continues the same
  conversation*. Hit a limit? Accounts → **move sessions** moves all of them at once.
- **Resources**: memory, CPU and process count per session (whole process tree, from `/proc`),
  machine free memory and kimo's own footprint (~5 MB RSS); a warning when memory runs low.
- **Remote access**: `--bind tailscale | zerotier | lan | <ip>`; a token is required whenever
  kimo is not on localhost.
- **Clean environment**: sessions do not inherit kimo's token or the state of a parent agent
  session (`CLAUDECODE`, `CLAUDE_CODE_*`), so kimo can itself be started from an agent.
- **A backend for your other PWAs**: per-app keys bound to the app's origin and to a scope
  (read or control), CORS, a live event stream and a JS SDK served by kimo (below).

## Run

Needs Linux and Rust (`curl https://sh.rustup.rs -sSf | sh`).

```bash
cargo run --release                         # http://127.0.0.1:7777, no token on localhost
cargo run --release -- --bind tailscale     # prints http://100.x.y.z:7777/#token=…
cargo test                                  # unit + PTY integration tests
```

Open the printed link on your phone once; the PWA stores the token. Settings can come from flags
or the environment (`.env.example`): `KIMO_BIND`, `KIMO_PORT`, `KIMO_TOKEN`, `KIMO_DATA_DIR`,
`KIMO_SCROLLBACK`, `KIMO_ORIGINS`.

**Install as an app (HTTPS):** browsers only install PWAs on HTTPS. With Tailscale, keep kimo
on localhost and let Tailscale terminate TLS: `tailscale serve --bg 7777`, then open
`https://<machine>.<tailnet>.ts.net` and "Add to home screen". Tailnet ACLs are then your auth;
set `KIMO_TOKEN` too if other people share the tailnet.

**Add an account:** Accounts → + → name and provider → **sign in** (opens a terminal running
`claude auth login` / `codex login` with that account's config home). Then pick the account
when creating a session, or switch it from the session row.

## Connect your PWAs

![An example app on another origin driving kimo's agents](docs/screenshots/app-agent-board.png)

1. **Give the app a key.** kimo → **Apps** → **+**: name, the app's origin (e.g.
   `https://pm.your-tailnet.ts.net`) and a scope: **control** (start, pause, stop, switch accounts,
   type, open terminals) or **read** (watch only). The key is shown once; revoke it anytime.
   Logins and keys themselves stay with the owner token.
2. **Load the SDK from kimo** and use the key:

```js
import { Kimo, quietFor } from 'https://kimo.your-tailnet.ts.net/sdk/kimo.js';
import 'https://kimo.your-tailnet.ts.net/sdk/kimo-terminal.js';   // defines <kimo-terminal>

const kimo = new Kimo({ url: 'https://kimo.your-tailnet.ts.net', token: 'kimo_…' });

kimo.subscribe(({ sessions, accounts }) => render(sessions));      // live, no polling
await kimo.createSession({ name: 'invoices', kind: 'claude', cwd: '~/dev/everpano' });
await kimo.send(id, 'run the tests and fix what fails');           // message an agent
await kimo.pause(id); await kimo.stop(id); await kimo.start(id);    // start = continue
await kimo.moveSessions('work-max', 'personal-pro');                 // subscription limit hit
quietFor(session);                                                   // seconds since last output

const term = document.querySelector('kimo-terminal');               // <kimo-terminal session="…">
term.kimo = kimo; term.setAttribute('session', id); term.send('/compact\r');
```

A key only works from the origins it lists, so a leaked key cannot be used from another
website, and no website can reach kimo without a key, even on localhost. Pages on HTTPS must reach
kimo over HTTPS too (`tailscale serve`). A complete example app, one static file, is in
[`examples/agent-board`](examples/agent-board): its own origin, its own look, live list,
pause/stop/continue, embedded terminal and message box.

| Agent board on a phone | Apps tab | New key |
|---|---|---|
| ![Agent board on a phone](docs/screenshots/app-agent-board-phone.png) | ![Apps tab](docs/screenshots/phone-apps.png) | ![New app key](docs/screenshots/phone-app-key.png) |

## API

All under `/api`, JSON, `Authorization: Bearer <key>` (or `?token=` for websockets and
EventSource). The owner token may do everything; app keys are limited by scope and origin.

| Method | Path | |
|---|---|---|
| GET | `/status` | memory, kimo RSS, counts |
| GET (SSE) | `/events` | `update` events with `{sessions, accounts}` on every change |
| GET, POST | `/sessions` | list; create `{name, kind: claude\|codex\|shell, cwd, account?, args?, command?, start?}` |
| GET, DELETE | `/sessions/{id}` | one session; delete (stops it first) |
| POST | `/sessions/{id}/action` | `{action: start\|pause\|resume\|stop\|restart}` |
| POST | `/sessions/{id}/account` | `{account: id\|null}`: switch subscription, continues the conversation |
| POST | `/sessions/{id}/input` | `{text}`: type into the terminal |
| GET (ws) | `/sessions/{id}/term` | terminal: binary frames both ways, text `{"type":"resize","cols","rows"}` |
| GET, POST | `/accounts` | list (with `signed_in`); create `{name, provider}` |
| DELETE | `/accounts/{id}` | delete (refused while sessions use it) |
| POST | `/accounts/{id}/login` | start a sign-in terminal |
| POST | `/accounts/{id}/move` | `{to: id\|null}`: move all its sessions (`default` = default login) |
| GET, POST | `/clients` | owner only: list app keys (no secrets); create `{name, origins, scope: read\|control}` → key |
| DELETE | `/clients/{id}` | owner only: revoke an app key |

Sessions carry `status`, `usage {rss_bytes, cpu_percent, processes}`, `last_output_at` (unix ms),
`account`, `kind`, `cwd`. SDK: `/sdk/kimo.js`, `/sdk/kimo-terminal.js` (loadable from any origin).

## Layout

```
src/main.rs            entry: flags/env → manager → HTTP server
src/api/               axum routes, websocket terminal, token auth, embedded PWA
src/services/          sessions (PTYs, lifecycle, switching), accounts (homes), store, stats, network
src/core/              pure logic: models, access rules, lifecycle, launch commands, /proc parsing, scrollback, bind parsing
web/                   kimo's PWA (web-timber style, xterm.js vendored), embedded in the binary on release
web/sdk/               the SDK other PWAs load from kimo: kimo.js (API + events), kimo-terminal.js
examples/agent-board/  a separate example app using only the SDK and an app key
tests/                 PTY integration tests with a fake `claude`; API access tests (CORS, keys, scopes, events)
```

## Limitations

- Codex sessions resume the folder's *last* Codex conversation (Codex has no way to set the
  id up front): avoid two Codex sessions in the same folder for now.
- Linux only for now (`/proc`, process groups).
- Sessions live inside the kimo process: stopping kimo stops the agents (they continue with
  *continue* afterwards). A detachable session holder is on the TODO list.
- `codex login` signs in through a browser callback on the machine itself; from a phone, sign
  Codex accounts in once at the computer or with `codex login --with-api-key`.
- The web layer uses the commercial Timber theme: keep the repository private.
