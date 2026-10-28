# Changelog

Changes since the last release collect under *Unreleased*; a release renames it to its
version, and that section becomes the GitHub release notes.

## Unreleased

- **kimo fleet**: a coordinator for several machines that each run kimo
  ([docs/fleet.md](docs/fleet.md)). `kimo-fleet` runs on an always-on machine (a Raspberry
  Pi) and:
  - keeps the registry of machines (`devices.yaml`);
  - wakes them with Wake-on-LAN and switches them off when idle (through their kimo, which
    checks first);
  - places jobs on the machine that fits: labels, VRAM, free GPU, already on before one to
    wake, cheapest first;
  - runs workflows whose steps pass files between machines and wait for your approval;
  - keeps logs, outputs and every event in SQLite, and notifies you through ntfy.
- A **Fleet** tab on the phone (when this kimo has a coordinator, `kimo serve --fleet`):
  - machines with their state, GPUs and load;
  - wake, switch off, keep on for 2 hours, manual control, pair;
  - jobs with their logs, outputs, cancel and approve;
  - workflows, and what happened lately;
  - a new job from a command or YAML.
- **One-way pairing** for coordinators (`"role": "fleet"`): the coordinator's key reaches
  only the machine's status, facts, jobs and power. It never reaches terminals, files,
  accounts, keys or other kimos, and never works from a web page.
- **Jobs** on every kimo (`/api/jobs`): a command or a headless agent (`claude -p`,
  `codex exec`) run to its end, detached from kimo.
  - Its state lives in files, so it survives kimo updating, restarting or crashing, and it
    ends with a real exit code.
  - Its log, a time limit, cancelling, and the agent's answer as its result.
  - It runs in a fresh folder, a folder of yours, or a fresh git checkout of a commit (a
    worktree or a clone, optionally on a new branch).
  - A job can be staged so input files go in first, and output files can be fetched after.
  - Running jobs keep the machine awake and on.
- `/api/node`: what the machine is and does now:
  - NVIDIA GPUs with their memory, driver and CUDA (`nvidia-smi`);
  - Jetson L4T, Raspberry Pi model, Hailo accelerator, Xcode;
  - load, disk, who is logged in, sessions and jobs;
  - labels made from all of this.
- `/api/power` and `/api/power/shutdown`: switch the machine off when nothing runs and nobody
  is logged in. Only the owner may force it.
- Paired kimos that do not answer (asleep, switched off) are left alone for 20 seconds
  instead of holding up every listing.
- SDK: `jobs()`, `createJob()`, `jobLog()`, `cancelJob()`, `startJob()`, `putJobFile()`,
  `jobFileUrl()`, `deleteJob()`, `node()`, `power()`, `powerOff()`, and `kimo.fleet.*`
  (`devices`, `wake`, `shutdown`, `hold`, `pair`, `events`, `samples`, `submit`,
  `submitWorkflow`, `log`, `approve`, `reject`, `cancel`, `artifactUrl`, `registry`).
- Releases include `kimo-fleet` next to `kimo`; the install script installs both.

## 0.1.14

- Updating no longer interrupts agents: the new kimo takes over the running terminals (the
  same processes, their screens, paused ones still paused), so Claude Code and Codex keep
  working through an update. A terminal that cannot be taken over starts again, continuing
  its conversation, as before. Right after an update kimo waits for its port to be free
  again instead of giving up.
- A right panel in the window (the button at the top right, ⌘⇧E), like Orca's: the
  project's **Files** (a tree with git's change letters, find by name or content), its
  **Changes** (staged and not, stage/unstage, commit, push, create a pull request with
  GitHub's `gh`, recent commits) and its **Agents** (running ones and earlier conversations
  to continue). Files open as tabs next to the terminals, to edit and save (⌘S); a copy
  changed on disk meanwhile (an agent) is not overwritten. Each file's changes show as a diff.
- On the phone, each project has *files* (browse and read) and *changes* (with the diffs).
- API and SDK: `/api/projects/{id}/files`, `/find`, `/file` (read and save), `/git`,
  `/git/diff`, `/git/log`, `/git/stage`, `/git/unstage`, `/git/commit`, `/git/push`,
  `/git/pr` (owner only, inside the project's folders); `files()`, `findFiles()`,
  `readFile()`, `writeFile()`, `gitStatus()`, `gitDiff()`, `gitLog()`, `gitStage()`,
  `gitUnstage()`, `gitCommit()`, `gitPush()`, `pullRequest()`.
- Plan limits are on by default and shown like Orca: every limit of the active subscriptions
  in the bottom bar ("Claude 4% 4 h 29 min · 41% 3 d 7 h · 15% Fable"), and each account's
  limits with bars in its panel. Codex's come live from Codex itself (`codex app-server`),
  not only from its logs; Claude's from Claude's usage API (macOS asks once to let kimo read
  Claude's login). The setting is now `plan_limits`.
- Each release's GitHub page carries its notes from this changelog (the in-app update card
  links to them).

## 0.1.13

- No *Terminals* button in the window's top bar: terminals open from their project or
  agent; clicking the open view's button again, or *kimo* next to the logo, goes back to
  them.

## 0.1.12

Security (from an audit of 0.1.7–0.1.11; please update):

- A request from this machine is its owner only when it is addressed to one of the
  machine's own names or addresses; other names need a key.
- Forgetting a paired kimo revokes the key it uses here and asks it to forget this kimo too;
  when it cannot be reached, kimo says to forget it there as well.
- Revoking a device, an app key or a paired kimo closes the terminals and event streams
  already open with that key (direct and through `/api/hosts/…`).
- `state.json` (it holds keys) is private from the moment it is written, and kimo's data
  folder is closed to other users.
- The SDK's `send()` and `sendFile()` (and the phone's photo upload) stay on the kimo chosen
  when they started, even if the shown kimo changes meanwhile.
- The install script refuses a download whose checksum it cannot check.
- Apps on other origins can use PUT and PATCH (renaming projects through the SDK).

Agents:

- Choose a new agent's model and effort on the phone (and through the API and SDK:
  `createSession({model, effort})`); the window's Settings have a default effort per agent.
  Claude gets `--model`/`--effort`, Codex `-m` and its reasoning effort; `/api/agents` lists
  the levels each accepts and model names to suggest.

## 0.1.11

- Terminals belong to projects: a terminal or agent started in a folder outside every project
  makes that folder a project (not the home folder itself), and at start the folders of older
  loose sessions become projects. The window's *+* shows inside a project only.
- Removing a project ends its terminals and agents (window, phone, `DELETE /api/projects/{id}`
  → `sessions_removed`); its folder stays, and conversations stay in the agents' history.
- The app icon style changes the Dock icon at once (macOS), without restarting kimo.
- No dark strip beside the projects panel when its content is wider than the panel.

## 0.1.10

- The projects panel resizes between 250 points (its content still fits, no dark gap) and
  half the window (560 at most).
- Settings → Appearance → app icon: light or dark, for the Dock icon while kimo runs and for
  kimo in Applications; updates keep the chosen style.
- Subscriptions say whether each account is signed in (✓ signed in / not signed in). *Sign
  in* shows only when needed; signing in again (an expired login) and removing are in the
  account's ··· menu (on the phone, a quiet "sign in again").

## 0.1.9

- Paste in the window's terminals uses the system clipboard: text copied in other apps pastes,
  and copying from the terminal reaches other apps. An image pasted into Claude Code or Codex
  on this machine reaches it as with any terminal ("[Image #1]"); on a paired kimo or in a
  shell it is saved on that machine and its path typed.
- Sessions: right-click one in the projects panel or its tab to open, stop or remove it.
  Sessions outside every project are grouped by folder, with *+ add as project*.
- The projects panel's fold button is right of the logo again, open or closed.
- The Dock icon stays the rounded app icon while kimo runs.

## 0.1.8

- The window's bottom bar: *Used* with the plan use of each provider's active subscription
  (percent and time to reset; all subscriptions in its panel), *awake* (keep the computer
  awake: on, while agents work, off), resources (memory and CPU per project and terminal,
  with a CPU line per project), ports (listening ports by the terminal or agent that opened
  them; the others folded) and the kimos (online state, switch, pair another).
- Phone app: the same as chips under the agents' figures, each with a sheet.
- The projects panel's fold button sits next to *+ kimo* (at the top left while hidden).
- Keep awake: `caffeinate` on macOS, `systemd-inhibit` on Linux, ended with kimo.
- API and SDK: `GET /api/ports` (`ports()`), `GET`/`POST /api/awake` (`awake()`,
  `setAwake(mode)`), `awake` in `/api/status`, `keep_awake` in the settings.
- Web files are revalidated on every load (ETags), so a phone runs the new app right after
  an update instead of up to an hour later.
- A phone that remembered a kimo no longer paired falls back to this one instead of an
  empty page.
- `kimo pair` without a running server says how to start it; `contrib/kimo.service` runs
  the kimo the install script puts in `~/.local/bin`.

## 0.1.7

- Updates: kimo checks for new versions every hour and says so once with a system
  notification, a bounce of the Dock icon, the card in the window and a banner on the phone.
  Closing the card leaves an *Update* button in the top bar. Settings → Updates: notify, or
  update by itself when no agent is working.
- A kimo server older than the window (left running from before an update or the install
  script) is offered a one-click restart onto the new version; servers before 0.1.2, which
  cannot restart themselves, are replaced and their running agents started again.
- The window and the phone app follow their server: when it runs a newer kimo (updated from
  the phone, by itself or by the install script) the window restarts on it and the phone app
  reloads.
- The Mac app and `~/.local/bin/kimo` both move to whichever of them is newest.
- `/api/update` says `restart` and `installed` when a newer kimo is on disk but not running.

## 0.1.6

- Mac app: kimo puts `kimo.app` in Applications (Dock, Launchpad, Spotlight) and keeps it and
  `~/.local/bin/kimo` at the same version; opened from the Dock it takes your login shell's
  PATH, so agents are found. `kimo install-app` does it by hand.
- Terminals per project: the tabs show the terminals of the selected project only, and *+*
  starts another terminal, Claude or Codex in it (⌘T: a terminal).
- The projects panel hides with the button at the top left (⌘B), for the terminal alone.
- Dropping: folders on the projects panel become projects; files dropped anywhere else go to
  the open terminal as paths.
- The views (Terminals, Usage, Subscriptions…) are buttons; the machines (this one and the
  paired kimos) are at the top of the projects panel.
- Subscriptions: grouped by provider, one *active* per provider where new agents start;
  *Use* asks once and can move the running agents too (they continue their conversations).
  No more "default login" rows: this computer's own login shows with its email. Plan limits
  with their pace: when each 5-hour and weekly limit runs out at the current rate (Codex from
  its logs, Claude from Claude's usage API once turned on).
- Usage: what the tokens cost at API prices, per model, day, project and subscription.
- Sign-in terminals ("login Work max") no longer stay in the sidebar once signed in.
- Devices: the QR code shows as soon as an address is chosen, with a short code
  (`K7M2-9QXP`) to type in the installed phone app.
- Phone app: installable (iPhone: Add to Home Screen, then type the code; Android: install
  over HTTPS), and it switches between every kimo paired with the one it uses (terminals
  too), through `/api/hosts/{host}/…`.
- API and SDK: `POST /api/accounts/active` (`setActiveAccount`), `/api/hosts/{host}/…`
  (`hosts()`, `useHost()`, `forHost()`), costs and `models` in `/api/usage/daily`, `pace` and
  Claude limits in `/api/usage`.

## 0.1.5

- Memory: when memory runs low, the window and the phone say so, list the biggest agents and
  stop the idle ones with one click. When memory is almost full kimo stops idle Claude and
  Codex agents by itself (biggest first; never working ones, ones asking you something, or
  shells) and tells you; they show as *asleep* and continue their conversation when opened.
- Settings → Memory: turn that off, stop agents idle for more than N minutes, and the size
  above which an agent shows as heavy.
- What an agent's memory is made of: click its memory figure to see the agent itself and each
  MCP server or tool it started (each Claude Code runs its own copies of your MCP servers).
- `GET /api/memory`, `POST /api/memory/free`, `GET /api/sessions/{id}/memory`; SDK
  `memory()`, `freeMemory()`, `sessionMemory(id)`.
- Fix: a `claude` or `codex` CLI that hangs on `--version` no longer blocks Settings and
  kimo's shutdown (10 s limit).

## 0.1.4

- Devices: choosing another address (Tailscale, ZeroTier, local network) redraws the QR code
  at once, with the same one-time code. Each address says what the phone needs to get through
  (for ZeroTier: the app joined to the same network and the phone authorized).
- macOS: kimo warns when the firewall blocks incoming connections for it (phones cannot
  connect) and says where to allow it; `GET /api/network` reports it as `firewall`.

## 0.1.3

- Phone: send photos to an agent. The camera button takes a photo, the image button picks
  from the gallery; large photos are shrunk to 2000 px before upload, shown as thumbnails,
  and their paths go into the message (Claude Code attaches images given by path).
  `POST /api/sessions/{id}/files` and `kimo.sendFile()` for your own apps; works for agents
  on paired machines too.

## 0.1.2

- Subscriptions: Claude accounts signed in on macOS showed as "not signed in" (Claude keeps
  the login in the Keychain there); kimo now reads the login Claude records, and shows its email.
- Terminal: resizing waits until the size settles before telling the agent (no more garbled
  redraws while dragging); the mouse wheel scrolls full-screen programs (Codex, less, vim);
  *redraw* reloads a garbled terminal and asks the program to draw itself again.
- Dark mode: Settings → Appearance (like the system, light, dark), for the window and the
  phone app; chart colours checked for both.
- Drop files or folders on the terminal to type their paths (Claude Code and Codex read
  images and files by path); drop a folder on the left to add it as a project.
- Opening a project continues its latest conversations, also when kimo sessions are already
  linked to them (they used to be skipped, so nothing opened); adding a known folder opens it.
- Projects like Orca: collapse, a ··· menu (settings, earlier conversations, show in
  Finder, remove) and + for a new git worktree with an agent in it; worktree cards group the
  agents of each branch. Project settings: display name and colour.
- Start from zero: folder pickers (window and phone) create new folders, and the empty
  terminal area has a "start a new project from zero" button (a new folder with git).
- Updates: the window shows when a new kimo is out; *Update* downloads it, checks it and
  restarts kimo on it, and agents that were running continue their conversations.

## 0.1.1

- macOS: `kimo serve` crashed at startup (stack overflow looking for the Tailscale CLI), so
  the window reported "the kimo server did not start". Fixed, and covered by a test that
  starts the real binary on Linux and macOS.
- Helper commands (ip, ifconfig, tailscale) have a 3 s time limit; the server answers before
  listing its addresses; when the server cannot start, the window shows the end of its log.

## 0.1.0

First public version.

- Native window (Rust, egui) with projects (drag and drop), terminals (alacritty_terminal),
  subscriptions, skills, settings and devices; a server that keeps agents running when the
  window closes.
- Claude Code, Codex and shell sessions; pause, stop and continue by conversation id.
- Adding a folder continues the Claude and Codex conversations that already happened there;
  every project has a history of earlier conversations (`kimo history`, `kimo continue`).
- Usage tab (window and phone), `kimo usage` and `GET /api/usage/daily`: tokens per day, per
  project and per subscription for every Claude and Codex conversation on the machine.
- Several Claude and Codex accounts; switch an agent's subscription and continue the same
  conversation; move every agent of an account at once; usage per account.
- Agent state (working, needs you, idle) and a "needs you" list.
- Agents working together: `kimo agents`, `send`, `read`, `wait`, `new`, and the bundled
  `kimo-agents` skill.
- Paired kimos on several machines: one window for all, agents addressed as `host/id`.
- Phone client (PWA) with QR pairing; HTTPS through Tailscale.
- HTTP API with live events, app keys bound to origins, a JS SDK and `<kimo-terminal>`.
- Linux and macOS; systemd unit, LaunchAgent, install script and release builds.
