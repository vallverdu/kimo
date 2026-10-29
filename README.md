<p align="center">
  <a href="assets/brand/kimo-dark.svg#gh-dark-mode-only"><img src="assets/brand/kimo-dark.svg" alt="kimo" width="96"></a><a href="assets/brand/kimo-light.svg#gh-light-mode-only"><img src="assets/brand/kimo-light.svg" alt="kimo" width="96"></a>
</p>

<h1 align="center">kimo</h1>

<p align="center"><b>A mobile-first agent development environment in Rust.</b><br>
Create projects and direct coding agents on your computers from your phone or tablet.<br>
Continue from any paired kimo, or build your own apps on the API.</p>

<p align="center"><a href="https://vallverdu.github.io/kimo-website/">Website</a> · <a href="https://vallverdu.github.io/kimo-website/docs.html">Documentation</a> · <a href="#install">Install</a> · <a href="docs/api.md">API &amp; SDK</a></p>

<p align="center">
  <a href="docs/screenshots/phone-projects-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-projects-dark.png" alt="Create projects and start agents on a selected computer from the phone PWA" width="270"></a><a href="docs/screenshots/phone-projects.png#gh-light-mode-only"><img src="docs/screenshots/phone-projects.png" alt="Create projects and start agents on a selected computer from the phone PWA" width="270"></a>
  <a href="docs/screenshots/phone-terminal-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-terminal-dark.png" alt="Direct an agent from the phone terminal" width="270"></a><a href="docs/screenshots/phone-terminal.png#gh-light-mode-only"><img src="docs/screenshots/phone-terminal.png" alt="Direct an agent from the phone terminal" width="270"></a>
</p>

<p align="center"><small>v0.1.15 · light and dark screenshots follow your GitHub theme · invented demonstration data</small></p>

Run kimo on a Mac mini, MacBook, Linux PC or GPU machine, then connect from its phone or
tablet PWA. Create a project from nothing on that host, start Claude Code or Codex in its
folder, and ask the agent to build it and publish it when you are ready. Agents, terminals
and builds use the host's resources; your phone is where you direct the work.

Pair your other kimos to continue that work from another computer while the project stays
on its original host. kimo keeps the Rust core small, shows resource pressure, and lets you
switch authenticated accounts while continuing the same conversations. Its shared API and
SDK also let you build your own PWA for cooperating agent teams.

With several machines, **kimo-fleet** coordinates them from one always-on board such as a
Raspberry Pi:

- it wakes GPU machines with Wake-on-LAN and switches them off when idle;
- it runs jobs and workflows on whichever machine fits;
- you watch and approve the work from the phone's Fleet tab ([docs/fleet.md](docs/fleet.md)).

The [product direction](docs/product-direction.md) records these principles, acceptance
scenarios and remaining gaps.

## What it does

- **Projects and terminals.** From the phone PWA, create a new project folder on the selected
  host, add an existing folder or clone a repository, then start Claude Code, Codex or a
  terminal in it. The native window also accepts dropped folders and has terminal tabs with
  selection and copy (⌘T for a terminal, ⌘B to hide the projects panel).
- **Model and effort per agent.** Choose Claude's or Codex's model and reasoning effort when
  starting an agent (phone and window), or set defaults per agent in Settings.
- **Worktrees.** *+* on a project creates a git worktree (a branch in its own folder) with an
  agent in it, so several agents work on the same repository without stepping on each other.
- **Files, changes and pull requests.** The window's right panel (⌘⇧E) has the project's
  **Files** (tree, find by name or content, open as editable tabs), **Changes** (diffs,
  stage, commit, push, create a pull request with `gh`) and **Agents**. On the phone each
  project has *files* and *changes*.
- **Images for agents.** Paste a screenshot into a terminal (it arrives as `[Image #1]`
  for Claude Code and Codex), or send photos from the phone's camera or gallery.
- **Pick up where you left off.** When a folder becomes a project, kimo finds the Claude Code
  and Codex conversations that already happened there and continues the latest ones, so you
  are back in your work at once. Every project has a *history* of its earlier conversations,
  each one a click away from continuing.
- **Subscriptions without logging out.** Add as many Claude and Codex accounts as you have
  and sign in once. One per provider is *active*: new agents start on it, and switching
  (one click and a yes) can move the running agents too, each **continuing the same
  conversation**. kimo shows every plan limit (5-hour, weekly, per model) of the active
  subscriptions, live, and **when it runs out at the current pace**.
- **Usage and cost.** A Usage tab shows the tokens of every Claude Code and Codex
  conversation on the machine, inside kimo or not, per day, per project (or repository), per
  subscription and per model, with **what it would cost at API prices**. Also on the phone
  and as `kimo usage`.
- **Pause, stop, continue.** Pause freezes an agent (no CPU); stop frees its memory; continue
  resumes its own conversation by id. Memory and CPU per agent, always visible, with a
  breakdown (the agent itself, each MCP server, builds). When memory runs low kimo warns you,
  and when it is almost full it stops idle agents, biggest first; they continue their
  conversation when you open them (Settings → Memory, also a timer for idle agents).
- **The host at a glance.** The window's bottom bar and phone controls show subscription
  use and reset times, keep-awake controls (on, while agents work, off), resources by
  project, listening ports by terminal and paired kimos. Open a control to inspect or manage
  the selected computer.
- **Jobs.** A command or a headless agent (`claude -p`, `codex exec`) can run to its end,
  detached from kimo. It survives kimo updating or crashing, ends with a real exit code, and
  keeps its log and the agent's answer. It runs in its own folder, a folder of yours, or a
  fresh git checkout of a commit (API `/api/jobs`, SDK `createJob`).
- **A fleet of machines.** `kimo-fleet` on an always-on machine knows your machines (GPUs,
  Jetson, Hailo, Xcode, load). It wakes them for work and switches them off when idle. It
  places jobs on the best one: one that is already on, otherwise the cheapest to keep on.
  Workflows pass files between steps on different machines and wait for your approval.
  See [Several machines](#several-machines).
- **Agents that work together.** Every agent has a clear id and resume id. Agents run
  `kimo agents`, `kimo send`, `kimo wait`, `kimo read` and `kimo new` to list, message, wait
  for and start other agents; the bundled `kimo-agents` skill teaches them how.
- **Know who needs you.** Each agent is *working*, *needs you* (asking a question or an
  approval) or *idle*. The window and the phone show what needs you first.
- **Several machines, one app.** Pair the kimo on your laptop with the ones on a GPU box, a
  Raspberry Pi or a Mac mini. Their projects and terminals appear in the same window (pick
  the machine at the top of the projects panel), and agents message each other across
  machines as `gpu-lab/trainer-1a2b`. The phone reaches every paired kimo through the one
  it is paired with.
- **Your phone.** Scan the QR code in Devices, or type its code in the installed app. kimo is
  an installable app (PWA) over your LAN, Tailscale or ZeroTier.
- **A Mac app.** kimo puts itself in Applications: open it from the Dock, Launchpad or
  Spotlight.
- **Your own apps.** Everything goes through a small HTTP API with live events. Give a web app
  its own key and it can create projects, start agents and orchestrate them with the JS SDK
  and an embeddable `<kimo-terminal>`.
- **Skills and settings.** See and manage your agents' skills (install from GitHub, a git URL
  or a folder); set permissions, model and arguments per agent; save quick commands.
- **Light or dark, always current.** Dark mode for the window and the phone, and a dark or
  light app icon. kimo tells you when a new version is out (a notification, a card in the
  window, a banner on the phone) and updates in one click, or by itself when no agent is
  working (Settings → Updates). **Updating never interrupts agents**: the new kimo takes
  over the running terminals.
- **Light.** One Rust binary, with a headless server build. Host, agent and kimo memory are
  reported separately so you can see the server’s own overhead.

## Install

Linux (x86_64, ARM64) and macOS (Apple silicon, Intel):

```bash
curl -fsSL https://raw.githubusercontent.com/vallverdu/kimo/main/install.sh | sh
# without the window, for headless machines (servers, Raspberry Pi):
curl -fsSL https://raw.githubusercontent.com/vallverdu/kimo/main/install.sh | sh -s -- --server
```

The script puts `kimo` (and `kimo-fleet`, the fleet coordinator) in `~/.local/bin`, checks
the release's SHA-256 first, and on macOS adds `kimo.app` to Applications.

From source (Rust stable):

```bash
cargo install --git https://github.com/vallverdu/kimo kimo
cargo install --git https://github.com/vallverdu/kimo kimo --no-default-features   # server only
cargo install --git https://github.com/vallverdu/kimo kimo-fleet                    # coordinator
```

kimo runs the agent CLIs you already have: install
[Claude Code](https://docs.anthropic.com/en/docs/claude-code) and/or
[Codex](https://github.com/openai/codex) first.

## Quick start

1. **Run kimo on the computer that will do the work:** `kimo serve` for a headless server,
   or `kimo` for the native window and background server. Closing the window leaves agents
   running. Install the agent CLIs and authenticate an account on the host first; Codex
   sign-in currently needs a browser on that machine.
2. **Connect your phone or tablet:** run `kimo pair` on the host, or show the QR code in
   Devices, and scan it. The devices need a reachable network such as your LAN, Tailscale
   or ZeroTier. See [Phone and web](#phone-and-web) to install the PWA and enable HTTPS.
3. **Create a project from the PWA:** open Projects → *+*, choose a new project, enter its
   name and create it. kimo makes the folder on the selected host. You can also add an
   existing host folder or clone a repository.
4. **Start and direct an agent:** choose *+ claude* or *+ codex* under the project, then send
   your task from its terminal/message box. Ask it to build and publish using the host's
   tools when needed. New agents use the active account; accounts can be switched while
   preserving their conversations.
5. **Connect other kimos:** run `kimo pair` on the other machine; in Devices → Other kimos,
   enter its address and code (or use `kimo peer add <address> <code>`). Select that machine
   to access its projects and agents from the PWA or native window.

In the native window, dropping a folder on the projects panel adds it. If Claude or Codex
already worked there, kimo continues their latest conversations; earlier ones are in the
project's *history*. Add more accounts in Subscriptions, and install the bundled
`kimo-agents` skill from Skills to teach agents how to work together.

## Agents working together

Inside any kimo session, agents (and you) can use:

```bash
kimo agents                          # ID, NAME, HOST, KIND, STATE, RESUME ID, FOLDER
kimo send reviewer "Review src/parser.rs and report problems"
kimo wait reviewer                   # until it is idle, needs input or stops
kimo read reviewer                   # its last answer, from its own transcript
kimo read reviewer --screen          # its recent terminal screen
kimo new claude --name reviewer --prompt "Review src/parser.rs"   # start a helper
kimo send gpu-lab/trainer-1a2b "Start the training run"           # an agent on another machine
```

Messages arrive signed: `[kimo message from agent coordinator (id mini/coordinator-0bffa3);
reply with: kimo send mini/coordinator-0bffa3 "…"]`, so the receiver knows who asks and how
to answer. The id is kimo's; the resume id is the agent's own conversation id (Claude's
session id, or the id Codex writes in its log). Both show in the window and in `kimo agents`.

## Several machines

Every kimo can pair with others (a one-time code; both sides learn about each other). Then:

- the machines at the top of the window's projects panel show each one's projects,
  sessions, subscriptions, skills and settings, and open their terminals directly;
- the phone (paired with any one of them) switches between them from its header;
- `kimo agents` lists the agents of every machine as `host/id`, and `send`, `read`, `wait` and
  the API's session calls work on them;
- kimos talk HTTP, so any network where they reach each other works: Tailscale, ZeroTier, your
  LAN, or SSH port forwarding in both directions (`ssh -L` and `-R`).

Give each machine a clear name with `--name` or `KIMO_NAME` (default: its hostname).

Paired kimos that do not answer (asleep, switched off) are left alone for 20 seconds, so
they do not slow the others down.

### A fleet: power, jobs and workflows

When some machines should sleep and others stay on, add the coordinator, `kimo-fleet`, on an
always-on machine. A Raspberry Pi 4 runs it in a few MB.

```
phone ─► kimo (Pi) ─/api/fleet─► kimo-fleet (Pi) ─fleet keys─► kimo on each machine
```

1. On the Pi: run `kimo-fleet serve` (`contrib/kimo-fleet.service`) and start its kimo
   with `KIMO_FLEET=http://127.0.0.1:7780`.
2. Describe the machines in `~/.kimo-fleet/devices.yaml`: address, MAC for Wake-on-LAN,
   power policy, labels, GPUs, idle watts.
3. Pair each one. On the machine run `kimo pair`; on the Pi run
   `kimo-fleet pair <id> <code>`, or tap *pair* in the Fleet tab. The pairing is **one
   way**: the coordinator's key reaches only that machine's facts, jobs and power, never its
   terminals, files or accounts. The machine gets no key for the Pi.
4. Run work with `kimo-fleet run job.yaml`, from the Fleet tab, or `kimo.fleet.submit()`:

```yaml
workflow: model-to-release
steps:
  - task: train
    requires: { labels: [cuda], vram_gb: 10 }     # wakes the NUC, switched off 20 min after
    run: python train.py --export onnx
    outputs: [out/model.onnx]
  - task: bench
    needs: [train]
    requires: { labels: [jetson] }
    inputs: { model: train/out/model.onnx }      # copied over, even with the NUC off
    run: ./bench_trt.sh out/model.onnx
  - task: approve-release
    runner: approval                              # waits for you in the Fleet tab
    needs: [bench]
```

By default:

- machines are never woken and never switched off; give a machine
  `power: { wake: wol, sleep: shutdown }` to change that;
- it is switched off after 20 idle minutes;
- a machine refuses to switch off while a terminal, a job or a logged-in person is on it;
- a wake gets 3 minutes and one retry;
- there are at most 10 automatic boots a day;
- each machine runs two jobs at once.

**Adding machines without sitting at them.** *Find machines* in the Fleet tab (or
`kimo-fleet discover`) lists what is on your Tailscale or ZeroTier network, whether kimo and
SSH answer there, and which hardware model it looks like. *Add* then works over SSH (your
keys or Tailscale SSH, never a password): it installs kimo as a service on that network,
pairs one way with a code made on the machine itself, and gives the machine its model
(NUC 11, Jetson Orin Nano, Raspberry Pi 5 with Hailo, Pi 4, Mac mini, MacBook Air, Linux PC)
and role (agent host, GPU worker, build worker, edge target, Apple builder, spare). Each model
sets labels, power policy, slots and idle watts; each role is a label jobs can ask for.

Everything is in [docs/fleet.md](docs/fleet.md).

### The map

The **Map** tab draws every machine and phone on your tailnet in 3D, to scale, live while it
is open: who is online, what each machine runs (projects, agents, facts, fleet jobs), and a
pulse when work starts or ends. Machines take the shape of their hardware model, and one that
is on the tailnet but not in the fleet can be added from there.

| Map | Fleet |
|---|---|
| <a href="docs/screenshots/phone-map-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-map-dark.png" alt="Every machine and phone on the tailnet in 3D"></a><a href="docs/screenshots/phone-map.png#gh-light-mode-only"><img src="docs/screenshots/phone-map.png" alt="Every machine and phone on the tailnet in 3D"></a> | <a href="docs/screenshots/phone-fleet-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-fleet-dark.png" alt="Machines, jobs and workflows in the Fleet tab"></a><a href="docs/screenshots/phone-fleet.png#gh-light-mode-only"><img src="docs/screenshots/phone-fleet.png" alt="Machines, jobs and workflows in the Fleet tab"></a> |

## Phone and web

The phone client is a web app served by kimo itself. It has:

- agents and projects, with files and changes;
- usage, accounts and devices;
- Fleet, when a coordinator sits beside this kimo, and the Map of your tailnet;
- a terminal with a key bar and a message box. Devices shows a QR code and a short code as soon
as you choose the address the phone uses (Tailscale, ZeroTier, Wi-Fi).

To install it as an app: on an iPhone, open the address in Safari, Share → Add to Home
Screen, then open kimo from the home screen and type the code (an installed app keeps its own
storage, so it pairs by itself). Android installs apps over HTTPS only: enable HTTPS in
Devices (`tailscale serve`, one click), scan the QR code and tap *install*.

| | | | |
|---|---|---|---|
| <a href="docs/screenshots/phone-agents-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-agents-dark.png" alt="Agents on a phone"></a><a href="docs/screenshots/phone-agents.png#gh-light-mode-only"><img src="docs/screenshots/phone-agents.png" alt="Agents on a phone"></a> | <a href="docs/screenshots/phone-terminal-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-terminal-dark.png" alt="A terminal on a phone"></a><a href="docs/screenshots/phone-terminal.png#gh-light-mode-only"><img src="docs/screenshots/phone-terminal.png" alt="A terminal on a phone"></a> | <a href="docs/screenshots/phone-usage-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-usage-dark.png" alt="Usage on a phone"></a><a href="docs/screenshots/phone-usage.png#gh-light-mode-only"><img src="docs/screenshots/phone-usage.png" alt="Usage on a phone"></a> | <a href="docs/screenshots/phone-history-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-history-dark.png" alt="Earlier conversations of a project on a phone"></a><a href="docs/screenshots/phone-history.png#gh-light-mode-only"><img src="docs/screenshots/phone-history.png" alt="Earlier conversations of a project on a phone"></a> |

## Build your own apps

Create a key for your app (web client → Apps, or `POST /api/clients`), limited to its origin
and to `read` or `control`, and load the SDK from kimo:

```js
import { Kimo } from 'https://kimo.your-tailnet.ts.net/sdk/kimo.js';
import 'https://kimo.your-tailnet.ts.net/sdk/kimo-terminal.js';   // <kimo-terminal session="…">

const kimo = new Kimo({ url: 'https://kimo.your-tailnet.ts.net', token: 'kimo_…' });
kimo.subscribe(({ sessions }) => render(sessions));                // live updates
const { project } = await kimo.createProject({ mode: 'new', name: 'invoices', git: true });
const agent = await kimo.createSession({ name: 'builder', kind: 'claude', cwd: project.path });
await kimo.send(agent.id, 'Create the invoice model with tests');
await kimo.waitFor(agent.id);                                       // until it stops working
console.log((await kimo.reply(agent.id)).text);                     // its answer
```

[`examples/agent-board`](examples/agent-board) is a complete app in one HTML file. The full
HTTP API, SDK and CLI reference is in [docs/api.md](docs/api.md).

## Run as a service

- **Linux (systemd user service):** `cp contrib/kimo.service ~/.config/systemd/user/`, then
  `systemctl --user enable --now kimo` and `loginctl enable-linger $USER`.
- **macOS (LaunchAgent):** `cp contrib/ai.kimo.plist ~/Library/LaunchAgents/`, then
  `launchctl load -w ~/Library/LaunchAgents/ai.kimo.plist`.
- **macOS app:** the install script (and the window, on its first start) puts `kimo.app` in
  Applications (or `~/Applications`), with the same binary; `kimo install-app` does it by
  hand. Updates keep the app and `~/.local/bin/kimo` at the same version. Opened from the
  Dock, kimo takes your login shell's PATH, so it finds `claude` and `codex`.
- A desktop launcher for Linux is in `contrib/kimo.desktop`.
- **The fleet coordinator:** `cp contrib/kimo-fleet.service ~/.config/systemd/user/`, then
  `systemctl --user enable --now kimo-fleet`, and add `KIMO_FLEET=http://127.0.0.1:7780` to
  kimo's environment on the same machine.

## Configuration

Flags or environment variables (see [.env.example](.env.example)):

| Variable | Default | |
|---|---|---|
| `KIMO_BIND` | `all` | where to listen: `all`, `localhost`, `tailscale`, `zerotier` or an IP |
| `KIMO_PORT` | `7777` | |
| `KIMO_NAME` | hostname | this machine's name for paired kimos |
| `KIMO_PROJECTS_DIR` | `~/dev` | where new projects are created |
| `KIMO_LOCAL_OWNER` | `true` | requests from this machine itself are the owner |
| `KIMO_TOKEN` | | optional owner token for scripts |
| `KIMO_DATA_DIR` | `~/.kimo` | state, account homes, jobs, backups |
| `KIMO_FLEET` | | the fleet coordinator on this machine, e.g. `http://127.0.0.1:7780` (shows the Fleet tab) |
| `KIMO_SHUTDOWN_COMMAND` | `systemctl poweroff` (Linux), System Events (macOS) | what `/api/power/shutdown` runs |
| `KIMO_SHUTDOWN_IGNORE_LOGINS` | | `1`: people logged in do not keep the machine on |

The coordinator has its own settings:

- `KIMO_FLEET_DIR` (default `~/.kimo-fleet`)
- `KIMO_FLEET_PORT` (default `7780`)
- `KIMO_FLEET_BIND` (default `127.0.0.1`)
- `KIMO_FLEET_TOKEN` (for scripts from elsewhere)

### Defaults

| | Default | Change it in |
|---|---|---|
| Listening | every network, port 7777 | `KIMO_BIND`, `KIMO_PORT` |
| Who is the owner | this machine itself, paired devices | `KIMO_LOCAL_OWNER`, Devices |
| New projects | `~/dev` | `KIMO_PROJECTS_DIR` |
| New agents | the active subscription; the CLI's own model and effort; the agent's own approval prompts (not yolo) | Subscriptions, Settings → Agents, or per agent |
| Plan limits | shown (Claude's usage API, Codex itself) | Settings |
| Keeping awake | off (the computer sleeps as usual) | bottom bar, Settings |
| Memory | idle agents stopped only when memory is critical; no idle timer; "heavy" above 1.5 GB | Settings → Memory |
| Updates | a notification per new version; installed only when you say | Settings → Updates |
| Colours | the system's light or dark | Settings → Appearance |
| Jobs | kimo keeps the newest 200; a job without a folder gets a fresh one; no time limit | the job's `cwd`, `git`, `timeout_s` |
| Fleet | coordinator only on 127.0.0.1:7780; machines never woken or switched off until `devices.yaml` says so | `devices.yaml` |

## Security model

- **The owner** is this machine itself (requests that come from it, not through a proxy,
  addressed by one of its own names), paired devices and paired kimos, and `KIMO_TOKEN` if
  you set one. The owner can do everything, including running commands on this machine
  through terminals.
- **A fleet coordinator** pairs one way and gets a key for the machine's status, facts,
  jobs and power only, never usable from a web page. It cannot force a shutdown. A job is a
  command run as you, so pair only your own coordinator.
- **Pairing** uses one-time codes valid for 10 minutes; every device and machine gets its own
  revocable key. Revoking a key closes its open terminals and streams at once.
- **App keys** are limited to their origins (CORS) and to `read` or `control`; they never
  manage logins, keys, devices, peers, skills or settings.
- Account credentials stay in each account's own config folder (`~/.kimo/accounts/<id>`);
  kimo's state file (keys included) is readable only by you.
- On a machine shared with other people, set `KIMO_LOCAL_OWNER=false` and pair your devices.

See [SECURITY.md](SECURITY.md) to report a vulnerability.

## More screenshots

v0.1.15 native window, with invented projects, conversations and usage. Images switch with
your GitHub theme; see [how they are made](docs/screenshots/README.md).

| Usage per day, project and subscription | A project’s agents and earlier conversations |
|---|---|
| <a href="docs/screenshots/app-usage-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-usage-dark.png" alt="Daily usage by project and subscription"></a><a href="docs/screenshots/app-usage.png#gh-light-mode-only"><img src="docs/screenshots/app-usage.png" alt="Daily usage by project and subscription"></a> | <a href="docs/screenshots/app-continue-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-continue-dark.png" alt="An existing project conversation open alongside its other agents"></a><a href="docs/screenshots/app-continue.png#gh-light-mode-only"><img src="docs/screenshots/app-continue.png" alt="An existing project conversation open alongside its other agents"></a> |
| **Earlier conversations of a project** | **Claude Code in kimo** |
| <a href="docs/screenshots/app-history-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-history-dark.png" alt="The history of a project"></a><a href="docs/screenshots/app-history.png#gh-light-mode-only"><img src="docs/screenshots/app-history.png" alt="The history of a project"></a> | <a href="docs/screenshots/app-claude-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-claude-dark.png" alt="Claude Code in the kimo window"></a><a href="docs/screenshots/app-claude.png#gh-light-mode-only"><img src="docs/screenshots/app-claude.png" alt="Claude Code in the kimo window"></a> |
| **Someone needs you** | **Subscriptions** |
| <a href="docs/screenshots/app-needs-you-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-needs-you-dark.png" alt="A session waiting for an answer"></a><a href="docs/screenshots/app-needs-you.png#gh-light-mode-only"><img src="docs/screenshots/app-needs-you.png" alt="A session waiting for an answer"></a> | <a href="docs/screenshots/app-subscriptions-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-subscriptions-dark.png" alt="Subscriptions"></a><a href="docs/screenshots/app-subscriptions.png#gh-light-mode-only"><img src="docs/screenshots/app-subscriptions.png" alt="Subscriptions"></a> |
| **Skills** | **Settings** |
| <a href="docs/screenshots/app-skills-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-skills-dark.png" alt="Skills"></a><a href="docs/screenshots/app-skills.png#gh-light-mode-only"><img src="docs/screenshots/app-skills.png" alt="Skills"></a> | <a href="docs/screenshots/app-settings-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-settings-dark.png" alt="Settings"></a><a href="docs/screenshots/app-settings.png#gh-light-mode-only"><img src="docs/screenshots/app-settings.png" alt="Settings"></a> |
| **A project's changes** | **A project's files** |
| <a href="docs/screenshots/app-changes-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-changes-dark.png" alt="A project's changes with a diff, stage and commit"></a><a href="docs/screenshots/app-changes.png#gh-light-mode-only"><img src="docs/screenshots/app-changes.png" alt="A project's changes with a diff, stage and commit"></a> | <a href="docs/screenshots/app-files-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-files-dark.png" alt="A project's files with a file open"></a><a href="docs/screenshots/app-files.png#gh-light-mode-only"><img src="docs/screenshots/app-files.png" alt="A project's files with a file open"></a> |
| **Devices and other machines** | **Accounts on the phone** |
| <a href="docs/screenshots/app-devices-dark.png#gh-dark-mode-only"><img src="docs/screenshots/app-devices-dark.png" alt="Devices"></a><a href="docs/screenshots/app-devices.png#gh-light-mode-only"><img src="docs/screenshots/app-devices.png" alt="Devices"></a> | <a href="docs/screenshots/phone-accounts-dark.png#gh-dark-mode-only"><img src="docs/screenshots/phone-accounts-dark.png" alt="Subscriptions and accounts on a phone" width="270"></a><a href="docs/screenshots/phone-accounts.png#gh-light-mode-only"><img src="docs/screenshots/phone-accounts.png" alt="Subscriptions and accounts on a phone" width="270"></a> |

## Platforms and limitations

- Linux and macOS. The macOS build is compiled and tested in CI; reports from real Macs are
  welcome.
- Agents run inside the kimo server. Updates hand them over to the new kimo, so they keep
  running. A crash or a reboot stops them, and they continue their conversation when
  started again. Jobs (`/api/jobs`) keep running through a crash too.
- Agent state (*working*, *needs you*) comes from output timing and the prompt shapes of
  Claude Code, Codex and common CLIs; unusual prompts may show as idle.
- Usage is counted in tokens from the agents' logs; costs are API list prices per model, an
  estimate of what the same work costs on the API (subscriptions are not billed per token).
  Codex's plan limits come from Codex itself, and Claude's from Claude's usage API. On
  macOS the Keychain asks once to let kimo read Claude's login; turn the limits off in
  Settings if you prefer.
  Which subscription a conversation used is known only for agents run by kimo; days are the
  local days of the machine the agents run on.
- `codex login` completes in a browser on the machine itself; sign Codex accounts in there once.
- Fleet: Wake-on-LAN, `systemctl poweroff` under a user service and the GPU, Jetson and
  Hailo facts are tested with stand-ins, not yet on real NUCs, Jetsons or Pis. Unattended
  shutdown on Linux may need a sudoers rule ([docs/fleet.md](docs/fleet.md)).

## Development

```bash
cargo run               # the window (starts a server in the background)
cargo run -- serve      # only the server
cargo test --workspace  # unit, API, PTY, two-server and fleet tests
cargo run -p kimo-fleet # the coordinator
```

Layout:

- `src/core`: pure logic
- `src/services`: sessions, projects, accounts, usage, peers, skills, jobs, node facts, power…
- `src/api`: HTTP and websockets
- `src/desktop`: the window
- `src/cli.rs`: `kimo agents`, `send`…
- `web/`: phone client and SDK
- `fleet/`: the coordinator, with the same layers

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT OR Apache-2.0, at your option. Bundled third-party components are listed in
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
