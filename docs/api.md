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
`/hosts`, `/update`, `/ports`, `/awake`, installing or removing skills, adding, signing in or deleting accounts.

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
| POST | `/sessions` | create `{name, kind, cwd?, account?, model?, effort?, args?, command?, start? = true}`; `model` and `effort` are this agent's own (Claude `--model`/`--effort`, Codex `-m`/reasoning effort; levels from `/agents`), kept for every start and resume; terminals belong to projects, so a `cwd` outside every project (and its worktrees) becomes a project named after the folder (not the home folder itself) |
| GET, DELETE | `/sessions/{id}` | one session; delete (stops it first) |
| POST | `/sessions/{id}/action` | `{action: start \| pause \| resume \| stop \| restart}` |
| POST | `/sessions/{id}/account` | `{account: id \| null}`: switch subscription; a running agent restarts and continues its conversation |
| POST | `/sessions/{id}/message` | `{text, from?}`: message an agent (typed as a paste, then Enter), signed with the sender session |
| GET | `/sessions/{id}/reply` | `{source: transcript \| terminal, text}`: the agent's last answer |
| GET | `/sessions/{id}/screen?lines=40` | recent terminal text without escape codes |
| GET | `/sessions/{id}/memory` | what its memory is made of: `[{pid, label, bytes, processes}]`, the agent first, then each process it started with its children (MCP servers, builds), biggest first |
| POST | `/sessions/{id}/input` | `{text}`: raw keystrokes |
| POST | `/sessions/{id}/files` | `{name, data}` (base64, up to 20 MB): save a file for the agent (a photo from the phone) in `~/.kimo/uploads/<session>/` → `{path}`; put the path in a message (Claude Code attaches images given by path) |
| GET (websocket) | `/sessions/{id}/term` | the terminal: binary frames both ways; text frame `{"type":"resize","cols":…,"rows":…}`; the recent output is replayed on connect |

## Live events

`GET /events` (server-sent events): an `update` event with `{sessions, accounts}` at connect
and on every change (status, account, activity, usage every ~2 s while agents run).

## Projects

| Method | Path | |
|---|---|---|
| GET | `/projects` | list with `color`, `branch`, `exists`, `sessions`, `running` and `worktrees` (`[{path, branch, primary}]`, the project's folder first) |
| POST | `/projects` | `{mode: new \| existing \| clone, name?, path?, git?, url?, resume? = true}` → `{project, session, continued, reopened}`: `session` is the clone terminal; `continued` are the folder's latest Claude and Codex conversations, continued as sessions (the first one started; sessions already attached to them are reused). Adding a folder that already is a project opens it: `reopened: true` |
| PATCH | `/projects/{id}` | `{name?, color?}`: rename, or set the sidebar colour (`#rrggbb`, `""` for none) |
| DELETE | `/projects/{id}` | forget it and end its terminals and agents → `{sessions_removed}`; files stay, conversations stay in the agents' history |
| GET | `/projects/{id}/files?dir=&root=` | a folder's entries `[{name, path, dir, size, ignored}]`, folders first (`.git` hidden; `ignored` by git). Owner only, like every files and git call; `root` picks one of the project's worktree folders; paths are relative and never leave the project (symlinks included) |
| GET | `/projects/{id}/find?q=&contents=&root=` | files whose path contains `q`, or with `contents=1` lines containing it (git grep): `[{path, line?, text?}]`, 200 at most |
| GET, PUT | `/projects/{id}/file?path=` | read `{path, text, binary, size, modified}` (`text` null when binary or over 2 MB); save `{path, text, modified?, root?}` → the saved file; `modified` from the read refuses a newer copy on disk (409) |
| GET | `/projects/{id}/git?root=` | `{repo, branch, upstream, ahead, behind, changes[{path, from?, letter: M\|A\|D\|R\|C\|U\|!, staged, unstaged}]}` |
| GET | `/projects/{id}/git/diff?path=&staged=` | `{diff}` (unified; a new file whole; 1 MB at most) |
| GET | `/projects/{id}/git/log?limit=` | recent commits `[{hash, subject, author, at}]` |
| POST | `/projects/{id}/git/stage`, `/git/unstage` | `{paths?, root?}` (no paths: everything) |
| POST | `/projects/{id}/git/commit` | `{message, root?}` → the commit |
| POST | `/projects/{id}/git/push` | `{root?}`: push the branch (its upstream set the first time) |
| POST | `/projects/{id}/git/pr` | `{title, body?, root?}` → `{url}`: a pull request with GitHub's `gh` |
| POST | `/projects/{id}/worktrees` | `{name, agent?: claude \| codex \| shell}` → `{worktree, session}`: `git worktree add` of branch `name` (made git-safe; an existing branch is checked out) in `~/.kimo/worktrees/<repository>/<branch>`, with an agent started in it |
| DELETE | `/projects/{id}/worktrees?path=` | `git worktree remove` (refused with uncommitted changes, or while agents run there; the branch stays); its stopped sessions are removed → `{sessions_removed}` |
| GET | `/fs?path=` | subfolders, for folder pickers |
| POST | `/fs` | `{parent, name}`: create a folder (start a project from nothing) → its listing |
| GET | `/conversations?path=` | earlier Claude and Codex conversations in a folder, newest first: `agent`, `id`, `title`, `updated_at`, `messages` (your prompts), `session` (when attached) |
| POST | `/conversations/continue` | `{agent, cwd, id, name?, start? = true}`: continue one as a session (an attached one is reused) |

## Accounts and usage

| Method | Path | |
|---|---|---|
| GET | `/accounts` | this computer's own logins first (`id: "system"`, `system: true`, named by their email; listed when signed in or used), then kimo's accounts; each with `signed_in`, `email` (the Claude login, when known), `sessions` and `active` (new agents of its provider start on it) |
| POST | `/accounts/active` | `{provider, account: id \| null \| "system", move?: bool}`: the subscription new agents of a provider start on; `move: true` also moves that provider's agents to it (running ones restart and continue their conversation) → `{moved}` |
| POST | `/accounts` | `{name, provider: claude \| codex}` |
| DELETE | `/accounts/{id}` | refused while sessions use it |
| POST | `/accounts/{id}/login` | start a sign-in terminal for it |
| POST | `/accounts/{id}/move` | `{to: id \| null}`: move every session of an account (`system` or `default` = this computer's own login) |
| GET | `/usage/daily?days=30` | every Claude and Codex conversation on the machine (in kimo or not), from the agents' logs, in local days: `days[] {date, tokens {claude, codex}, prompts, cache_read}`, `places[] {name, path, project, tokens, prompts, conversations, last_active}` (folders grouped by kimo project, else by git repository), `accounts[] {account, name, provider, tokens, prompts, cost}`, `outside_kimo`, `today`, `last_7_days`, `total`, `prompts`, `conversations`, `cost_today`, `cost_7_days`, `cost_total`, `models[] {model, provider, tokens {input, output, cache_write, cache_read}, answers, cost}`. Tokens = new input + output + cache writes; costs are US dollars at API list prices per model (days, places and accounts carry `cost` too) |
| GET | `/usage` | per account: `tokens_5h`, `tokens_7d`, `detail_7d`, `messages_7d`, `last_activity`, `limits {limits[] {name, used_percent, window_minutes, resets_at, pace {projected_percent, runs_out_at}}, reached}` (Codex live from Codex itself, `codex app-server`, else from its logs; Claude from Claude's usage API; both while the `plan_limits` setting is on, the default), `limits_note` (why a login's limits are not shown). `pace`: the percent used by the reset at the current rate and, when that passes 100, when it runs out (unix seconds) |

## Skills, settings, agents

| Method | Path | |
|---|---|---|
| GET | `/skills?project=` | skills: `name`, `description`, `agent` (`claude`, `codex`, `shared`), `scope`, `path`, `managed` (read-only) |
| POST | `/skills` | install `{source, skill?, agents, project?, replace?}` from `owner/repo/folder`, a git URL or a folder |
| DELETE | `/skills?path=` | remove (moved to `~/.kimo/skill-backups`) |
| POST | `/skills/kimo` | install the bundled `kimo-agents` skill `{agents?}` |
| GET, PUT | `/settings` | `agents.{claude,codex}.{enabled, permissions: manual \| yolo, model, effort, args}`, `terminal.font_size`, `quick_commands[{name, text, project?}]`, `theme: system \| light \| dark` (window and web client), `updates.{notify, auto}` (a system notification once per new version; installing and restarting by itself when no agent is working), `keep_awake: off \| agents \| on` |
| GET | `/agents` | agent CLIs found on the machine: `[{kind, installed, version, efforts, models}]` (the effort levels each accepts, model names to suggest) |

## Devices, network, other kimos

| Method | Path | |
|---|---|---|
| POST | `/pairing` | a one-time pairing code (10 minutes), short enough to type: `K7M2-9QXP` |
| POST | `/pair` | `{code, name}` → a device key (no key needed: the code is the proof). Case, spaces and dashes in typed codes do not matter; after 20 wrong codes every open code stops working |
| GET, DELETE | `/devices`, `/devices/{id}` | paired devices; unpair |
| GET | `/network` | addresses kimo is reachable on; Tailscale name and HTTPS address; `firewall`: why phones may be blocked (macOS firewall), when that is the case |
| POST | `/network/https` | serve kimo over HTTPS on the tailnet (a free port; other services untouched) |
| GET | `/qr?text=` | a QR code as SVG |
| GET | `/peers` | paired kimos with `online` and `sessions` |
| POST | `/peers` | `{url, code, my_url?}`: pair with another kimo using a code shown there |
| DELETE | `/peers/{id}` | forget a paired kimo |
| GET | `/peers/keys` | paired kimos with their keys (for the owner's own apps) |
| POST | `/peers/accept` | used by another kimo during pairing |
| any | `/hosts/{host}/…` | the same API on a paired kimo (`host`: its id or name from `/peers`), through this one with this kimo's key there: calls, `/events` and terminal websockets. A phone paired with one kimo uses all of them |

## The machine

| Method | Path | |
|---|---|---|
| GET | `/ports` | listening TCP ports: `[{port, addresses, pid, process, session, session_name, kimo}]`; `session` is the kimo session whose processes listen there (a dev server an agent started), `kimo` marks kimo itself; ports of the system and other users come without a process |
| GET | `/awake` | keeping the computer awake: `{mode: off \| agents \| on, active, supported}` (`caffeinate` on macOS, `systemd-inhibit` on Linux); also in `/status` as `awake` |
| POST | `/awake` | `{mode}`: `agents` keeps it awake while an agent works and two minutes after; saved as `keep_awake` in the settings |

## Updates

| Method | Path | |
|---|---|---|
| GET | `/update?force=` | the latest release on GitHub (kimo checks every hour in the background): `{current, latest, available, installed, restart, url, notes, manual, error}`; `restart` means a newer kimo (`installed`) is on disk already, put there by an update or the install script, and restarting runs it; `available` is a newer release still to download; `manual` says why kimo cannot install it itself (built from source, a folder you cannot write) |
| POST | `/update` | download the release for this machine, check its SHA-256 and put it in place of kimo's binary → `{installed}` |
| POST | `/update/restart` | restart kimo on the installed version (same process): running terminals and agents are handed to the new kimo and keep running (paused ones stay paused); one that cannot be handed over starts again, continuing its conversation |

## App keys

| Method | Path | |
|---|---|---|
| GET | `/clients` | app keys (without secrets) |
| POST | `/clients` | `{name, origins, scope: read \| control}` → the key, shown once |
| DELETE | `/clients/{id}` | revoke |
| GET | `/status` | `host`, `version`, memory (`memory_pressure`: `ok`, `low`, `critical`), kimo's own memory |
| GET | `/memory` | `total_bytes`, `available_bytes`, `agents_bytes`, `pressure`, `low_bytes`, `critical_bytes`, `heavy` (agent ids over the setting), `idle` (agents that can be stopped now), `freed[] {id, name, at, bytes, reason}` (agents kimo stopped: `low memory`, `idle` or `asked`) |
| POST | `/memory/free` | stop every idle Claude and Codex agent now; `{stopped: [ids]}`. They continue their conversation when started (stopped agents carry `freed`) |

## JS SDK

Served by kimo at `/sdk/kimo.js` (loadable from any origin):

```js
import { Kimo, quietFor } from 'https://<kimo>/sdk/kimo.js';
const kimo = new Kimo({ url: 'https://<kimo>', token: 'kimo_…' });

kimo.sessions(); kimo.session(id); kimo.createSession(spec); kimo.deleteSession(id);
kimo.start(id); kimo.pause(id); kimo.resume(id); kimo.stop(id); kimo.restart(id);
kimo.setAccount(id, account);       // switch subscription, continues the conversation
kimo.send(id, text);                // type a message and Enter
kimo.sendFile(id, file);            // a photo or file for the agent → {path}
kimo.message(id, text, from);       // like `kimo send`, signed with a sender session
kimo.reply(id); kimo.screen(id, lines);
kimo.memory(); kimo.freeMemory(); kimo.sessionMemory(id);
kimo.waitFor(id, ['idle', 'waiting'], { timeout });   // resolves with the session
kimo.subscribe(({ sessions, accounts }) => …);        // live updates; returns stop()
kimo.projects(); kimo.createProject(p); kimo.updateProject(id, { name, color });
kimo.deleteProject(id); kimo.browse(path); kimo.makeFolder(parent, name);
kimo.addWorktree(id, { name, agent }); kimo.removeWorktree(id, path);
kimo.conversations(path); kimo.continueConversation({ agent, cwd, id });
kimo.accounts(); kimo.usage(); kimo.dailyUsage(days); kimo.moveSessions(from, to);
kimo.setActiveAccount(provider, account, { move });   // new agents start on it
kimo.hosts();                                           // paired kimos (owner only)
kimo.useHost(id); kimo.forHost(id);                     // talk to one of them through this kimo
kimo.settings(); kimo.saveSettings(s);                   // owner only
kimo.release(force); kimo.installUpdate(); kimo.restartKimo();   // owner only
kimo.ports(); kimo.awake(); kimo.setAwake('agents');          // owner only
quietFor(session);                  // seconds since its last output
```

`/sdk/kimo-terminal.js` defines `<kimo-terminal session="…">`: set `element.kimo = kimo`
(or `url` and `token` attributes); `send(text)` types into it; `readonly` watches only. Its
colours follow the page's `--color-*` tokens and its light or dark scheme (`<html data-theme>`).

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
kimo usage [--days 14] [--json]       tokens per day, project and subscription
kimo history [folder]                 earlier Claude and Codex conversations here
kimo continue <conversation> [--folder DIR]
kimo peers
kimo peer add <url> <code> [--my-url URL]
kimo peer remove <id>
```

Inside kimo sessions `KIMO_URL` and `KIMO_SESSION` are set, so commands reach the right kimo
and messages are signed with the sending agent.
