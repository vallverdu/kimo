# kimo API, SDK and CLI

Everything kimo does goes through this API: the window, the phone client, the CLI and your
own apps use the same calls.

## Access

All calls are under `/api`, with JSON bodies. Send a key as `Authorization: Bearer <key>`, or
as `?token=<key>` where headers are not possible (websockets, EventSource).

| Caller | Gets in with | May do |
|---|---|---|
| This machine itself | nothing (requests from the machine, not through a proxy; `KIMO_LOCAL_OWNER`) | everything |
| A paired device or kimo | its device key (pairing) | everything |
| A script | `KIMO_TOKEN` | everything |
| An app | an app key (`/clients`), only from its listed origins | `control`: sessions, projects, messages, terminals, account switching · `read`: GET only, no terminals, no folder listing |

Owner only: `/clients`, `/devices`, `/pairing`, `/network`, `/qr`, `/settings`, `/peers`,
installing or removing skills, adding, signing in or deleting accounts.

Requests from another web page (an `Origin` that is not kimo itself) need a key that lists
that origin; kimo answers CORS preflights for listed origins.

## Sessions

A session is a terminal running Claude Code, Codex or a shell. Fields: `id`, `name`, `kind`
(`claude`, `codex`, `shell`), `cwd`, `account` (null = default login), `conversation` (resume
id), `status` (`running`, `paused`, `stopped`, `exited`), `activity` (`working`, `waiting`,
`idle` while running), `usage {rss_bytes, cpu_percent, processes}`, `last_output_at` (unix ms),
`args`, `command`, `pid`, `exit_code`. With `?all=1`, sessions of paired kimos are included,
with ids `host/id` and fields `host` and `local`.

Wherever a session id goes, a session name or a unique id prefix works too, and `host/id`
reaches a session on a paired kimo.

| Method | Path | |
|---|---|---|
| GET | `/sessions` | list; `?all=1` adds the sessions of paired kimos |
| POST | `/sessions` | create `{name, kind, cwd?, account?, args?, command?, start? = true}` |
| GET, DELETE | `/sessions/{id}` | one session; delete (stops it first) |
| POST | `/sessions/{id}/action` | `{action: start \| pause \| resume \| stop \| restart}` |
| POST | `/sessions/{id}/account` | `{account: id \| null}`: switch subscription; a running agent restarts and continues its conversation |
| POST | `/sessions/{id}/message` | `{text, from?}`: message an agent (typed as a paste, then Enter), signed with the sender session |
| GET | `/sessions/{id}/reply` | `{source: transcript \| terminal, text}`: the agent's last answer |
| GET | `/sessions/{id}/screen?lines=40` | recent terminal text without escape codes |
| POST | `/sessions/{id}/input` | `{text}`: raw keystrokes |
| GET (websocket) | `/sessions/{id}/term` | the terminal: binary frames both ways; text frame `{"type":"resize","cols":…,"rows":…}`; the recent output is replayed on connect |

## Live events

`GET /events` (server-sent events): an `update` event with `{sessions, accounts}` at connect
and on every change (status, account, activity, usage every ~2 s while agents run).

## Projects

| Method | Path | |
|---|---|---|
| GET | `/projects` | list with `branch`, `exists`, `sessions`, `running` |
| POST | `/projects` | `{mode: new \| existing \| clone, name?, path?, git?, url?}` → `{project, session}` (`session` is the clone terminal) |
| DELETE | `/projects/{id}` | forget (files stay) |
| GET | `/fs?path=` | subfolders, for folder pickers |

## Accounts and usage

| Method | Path | |
|---|---|---|
| GET | `/accounts` | list with `signed_in` and `sessions` |
| POST | `/accounts` | `{name, provider: claude \| codex}` |
| DELETE | `/accounts/{id}` | refused while sessions use it |
| POST | `/accounts/{id}/login` | start a sign-in terminal for it |
| POST | `/accounts/{id}/move` | `{to: id \| null}`: move every session of an account (`default` = default login) |
| GET | `/usage` | per account: `tokens_5h`, `tokens_7d`, `detail_7d`, `messages_7d`, `last_activity`, `limits` (Codex: `used_percent`, `window_minutes`, `resets_at`) |

## Skills, settings, agents

| Method | Path | |
|---|---|---|
| GET | `/skills?project=` | skills: `name`, `description`, `agent` (`claude`, `codex`, `shared`), `scope`, `path`, `managed` (read-only) |
| POST | `/skills` | install `{source, skill?, agents, project?, replace?}` from `owner/repo/folder`, a git URL or a folder |
| DELETE | `/skills?path=` | remove (moved to `~/.kimo/skill-backups`) |
| POST | `/skills/kimo` | install the bundled `kimo-agents` skill `{agents?}` |
| GET, PUT | `/settings` | `agents.{claude,codex}.{enabled, permissions: manual \| yolo, model, args}`, `terminal.font_size`, `quick_commands[{name, text, project?}]` |
| GET | `/agents` | agent CLIs found on the machine, with versions |

## Devices, network, other kimos

| Method | Path | |
|---|---|---|
| POST | `/pairing` | a one-time pairing code (10 minutes) |
| POST | `/pair` | `{code, name}` → a device key (no key needed: the code is the proof) |
| GET, DELETE | `/devices`, `/devices/{id}` | paired devices; unpair |
| GET | `/network` | addresses kimo is reachable on; Tailscale name and HTTPS address |
| POST | `/network/https` | serve kimo over HTTPS on the tailnet (a free port; other services untouched) |
| GET | `/qr?text=` | a QR code as SVG |
| GET | `/peers` | paired kimos with `online` and `sessions` |
| POST | `/peers` | `{url, code, my_url?}`: pair with another kimo using a code shown there |
| DELETE | `/peers/{id}` | forget a paired kimo |
| GET | `/peers/keys` | paired kimos with their keys (for the owner's own apps) |
| POST | `/peers/accept` | used by another kimo during pairing |

## App keys

| Method | Path | |
|---|---|---|
| GET | `/clients` | app keys (without secrets) |
| POST | `/clients` | `{name, origins, scope: read \| control}` → the key, shown once |
| DELETE | `/clients/{id}` | revoke |
| GET | `/status` | `host`, `version`, memory, kimo's own memory |

## JS SDK

Served by kimo at `/sdk/kimo.js` (loadable from any origin):

```js
import { Kimo, quietFor } from 'https://<kimo>/sdk/kimo.js';
const kimo = new Kimo({ url: 'https://<kimo>', token: 'kimo_…' });

kimo.sessions(); kimo.session(id); kimo.createSession(spec); kimo.deleteSession(id);
kimo.start(id); kimo.pause(id); kimo.resume(id); kimo.stop(id); kimo.restart(id);
kimo.setAccount(id, account);       // switch subscription, continues the conversation
kimo.send(id, text);                // type a message and Enter
kimo.message(id, text, from);       // like `kimo send`, signed with a sender session
kimo.reply(id); kimo.screen(id, lines);
kimo.waitFor(id, ['idle', 'waiting'], { timeout });   // resolves with the session
kimo.subscribe(({ sessions, accounts }) => …);        // live updates; returns stop()
kimo.projects(); kimo.createProject(p); kimo.deleteProject(id); kimo.browse(path);
kimo.accounts(); kimo.usage(); kimo.moveSessions(from, to);
quietFor(session);                  // seconds since its last output
```

`/sdk/kimo-terminal.js` defines `<kimo-terminal session="…">`: set `element.kimo = kimo`
(or `url` and `token` attributes); `send(text)` types into it; `readonly` watches only.

## CLI

```text
kimo                 open the window (starts a server in the background if needed)
kimo serve           run only the server
kimo pair            print a QR code to pair a phone or another kimo
kimo agents [--json] list agents of this and paired kimos
kimo send <id> <message…>
kimo read <id> [--screen] [--lines N]
kimo wait <id> [--timeout S]
kimo new <claude|codex|shell> [--name N] [--cwd DIR] [--account A] [--prompt TEXT]
kimo whoami
kimo peers
kimo peer add <url> <code> [--my-url URL]
kimo peer remove <id>
```

Inside kimo sessions `KIMO_URL` and `KIMO_SESSION` are set, so commands reach the right kimo
and messages are signed with the sending agent.
