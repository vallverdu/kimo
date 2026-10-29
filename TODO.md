# TODO

## Product priorities (owner direction, 2026-10-26)

Follow [docs/product-direction.md](docs/product-direction.md) and [AGENTS.md](AGENTS.md).
The main goal is creating and directing a new project from a phone PWA on a remote computer,
then continuing it from any paired device or kimo. This section sets priority; the existing
sections below track implementation by area and do not rank desktop work first.

- [ ] Verify the full phone journey on a real device: pair with a configured headless host,
  create a new folder/project, choose an agent/account, send a task, review output, answer
  questions and direct publishing when requested. Record the client and host used.
- [ ] Simplify fresh project creation into a guided PWA flow with the agent, account and
  initial task; creation and launch currently work as separate steps. Expose shared behaviour
  through the API and SDK.
- [ ] Verify phone sleep/network reconnect without stopping the agent, and continuation of
  the same host project/conversation from another client or paired kimo.
- [ ] Verify project creation and terminal/events routing on the selected peer; finish
  HTTPS/WSS support in the Rust clients for encrypted peer addresses.
- [ ] Finish mobile account setup and limit recovery; verify conversation-preserving account
  switching with real Claude Code and Codex accounts, without logout/login or manual resuming.
- [ ] Verify memory warnings and recovery from the phone, with measured kimo overhead under
  several sessions. Explain that pause saves CPU and stopping a resumable agent frees RAM.
- [ ] Finish the PWA settings/skills controls needed to operate away from the host.
- [ ] Verify the same create-project → run-agent → message/read/observe workflow from a
  separate app using the SDK and an appropriately scoped key.

These are workflow improvements and acceptance checks; they do not undo completed backend
tasks below. Keep each area current as work lands. Optional desktop polish follows these
priorities. Keep the core small and projects as ordinary folders, with kimo state outside them.

## Requirements audit (2026-10-29)

See [docs/requirements-audit-2026-10-29.md](docs/requirements-audit-2026-10-29.md) for the
acceptance matrix, reproductions and verification limits. The starting workspace passed
formatting, Clippy, 178 tests (1 live-provider test ignored) and the headless build; this does
not close the workflow checks above.
The findings were sent to the existing main Claude terminal through Orca; input acceptance
was confirmed. Fixes and the remaining acceptance work are open.

- [x] Reconcile uncertain fleet dispatches before failover (F1). A run's id is written down
  before sending; an unclear answer is settled by asking the machine about that id (adopted,
  placed again under the same id, or kept as uncertain with its lease). Runs that may still
  be alive (lost machines, late arrivals) are cancelled when their machine answers, their
  GPU and slots held until then. A restarted coordinator checks jobs that were on their way,
  and a half-uploaded staged run is cancelled (it can then never start).
  Tests: `a_lost_answer_never_runs_a_job_twice` (proxy drops the 201), `a_restarted_coordinator_settles_jobs_that_were_on_their_way`, `a_cancelled_staged_job_never_starts`.
- [x] Guard shutdown against work started during its delay (F2). There is a last check right
  before the command. New work wins and the shutdown stops with a reason; past the last check,
  new terminals and jobs are refused, and still being up 30 s later lifts that. Test:
  `work_started_while_switching_off_stops_the_shutdown` (a terminal and a job; stand-in command).
- [x] Missing workflow inputs (F3): an input must be a declared output of its step
  (refused at submission), and a step whose input was not made fails instead of running.
  Test: `a_step_never_runs_without_its_inputs`.
- [x] Cancelling an unreachable job (F4): it stays *cancelling* with its lease until its
  machine confirms the run ended; the cancel is retried when the machine answers.
  Test: `cancelling_waits_for_the_machine_to_confirm`.
- [x] Stale PWA host answers (F5): a host generation. Answers, the event stream and the
  status poll of a previously selected kimo are dropped, and created sessions open only on
  their own kimo. Verified in Chrome with fixtures (A's projects 3 s late, B selected: B
  stays). No automated browser regression.
- [x] Terminal drafts during disconnection (F6): `<kimo-terminal>.send()` reports
  delivery. The composer keeps the text and images and disables Send while reconnecting,
  and Enter is never replayed by itself. Verified in Chrome against a `cat` terminal (closed
  socket: draft kept; reconnected: delivered once). No automated browser regression.
- [x] Live capacity is authoritative (F7, `schedule` unit test). TLS for notifications and
  ZeroTier Central (F8): HTTPS works and failures are recorded as events
  (`notifications_arrive_and_failures_are_recorded`). Approval evidence from `show:` is
  resolved to earlier steps' outputs, shown with previews in the Fleet tab
  (`a_workflow_passes_files_between_machines_and_waits_for_approval`).
- [ ] Still open from the audit: guided phone creation, limit recovery, phone
  account/settings/skills, real providers and devices, overhead benchmarks. Its reproduction
  harness was not rerun against these fixes; the regressions above were written from its
  reproductions.

## Review and README presentation (2026-10-26)

- [x] Extend the separate website with a [documentation page](https://vallverdu.github.io/kimo-website/docs.html):
  15 sections covering setup, phone pairing and project creation, everyday workflows,
  accounts, resources, settings, custom apps, API/CLI, configuration/services and troubleshooting.
  Source review includes installer differences, owner/app access and unfinished peer TLS,
  account setup and process continuity. Add mobile contents, reading focus, section tracking,
  copyable examples, cross-page links and build-time anchor checks. Chrome checked ten widths
  (320–1440), light/dark, navigation/keyboard/clipboard, reduced motion and no-JavaScript reading.
  This verifies the documentation UI; real devices, providers, installer execution and actual
  host/API/SDK workflows were not exercised. Website source and deployment remain in `../kimo-website`.
- [x] Build and publish the one-page [website](https://vallverdu.github.io/kimo-website/)
  in the separate [kimo-website repository](https://github.com/vallverdu/kimo-website),
  with a sibling local checkout at `../kimo-website`. Reuse the jordivallverdu.com template
  grid, Roboto Mono and kimo tokens; include public demo screenshots, light/dark themes,
  the brand agent's SVG animations, an SDK introduction and install commands. GitHub Pages
  deploys automatically from its own workflow. Chrome checks covered nine widths (320–1440),
  menu/gallery keyboard controls, theme persistence, copy commands, reduced motion and
  no-JavaScript content; Safari and real devices were not checked. This is website
  verification; the API/SDK and host routing did not change and product acceptance remains
  open above. The website README records asset provenance and the explicit asset-sync step.
- [x] Update the GitHub About text to the mobile-first product direction; refresh v0.1.8
  README screenshots in light and dark, with pure white/black logo variants and GitHub
  theme selection. Capture conditions and verification are in `docs/screenshots/README.md`.
  The About website is set to `https://vallverdu.github.io/kimo-website/` (verified via GitHub).
  The captures use demo data and phone emulation; real-device/provider/peer acceptance
  checks above remain open.
- [x] Review Herdr's documentation and the independent wherdr phone companion, including
  its scripted public demo at a touch viewport. Record the proposed order and acceptance
  checks in [docs/mobile-workflow-review.md](docs/mobile-workflow-review.md): trust/routing,
  guided creation, readable conversations, attention/notifications, recovery and mobile
  review. These are recommendations; implementation and real-device acceptance remain open.
- [ ] Review the private local audit handoff for the Claude agent at
  `.git/kimo-audit-2026-10-26.md`: prioritize owner access validation, credential revocation
  and private state writes; also check SDK cross-origin methods and bounded process I/O.
  The audit and mobile review were delivered through Orca to the existing main Claude
  terminal on 2026-10-26; the receipt confirmed acceptance and a started Claude turn.
  The review also reproduced mutable-host routing in SDK sends/uploads on v0.1.11.
  Delivery does not mark the security fixes or proposed workflows complete.

## Brand artwork and splash motion (2026-10-26)

- [x] Preserve the existing brush-outline logo through VectorCraft MCP; save the editable
  `assets/brand/kimo.ai` master, then export `kimo.svg`. Add separate bun/stick/head artwork
  in `kimo-animation.ai` and `kimo-parts.svg`; both AI files reopen in VectorCraft without warnings.
- [x] Prepare a transparent 4.2-second SVG splash: long hair lifts and twists into the bun,
  the stick slides through and the logo settles. Add an offline preview with replay/scrub/theme
  controls and the identical PWA asset at `/icons/kimo-splash.svg`. Chrome desktop/phone
  emulation, light/dark, reduced motion and the final silhouette were checked; Rust checks and
  the headless build pass. See `assets/brand/README.md`. Startup overlay integration, Adobe
  Illustrator, Safari and real-device checks were not performed; OS-managed PWA launch screens
  remain static.
- [x] Add `kimo-eyes.svg`: the finished bun, stick and head stay fixed while both eyes move
  gently up and down in a 4.8-second loop. Include the PWA copy, offline playback preview and
  GIF review export. Chrome geometry/loop-seam checks, light/dark phone emulation, reduced
  motion and the original splash preview passed; Safari and real-device checks remain open.
- [x] Add the connected-kimo animation (2026-10-27): a phone, laptop kimo and headless kimo
  exchange signals along curved links, with arrival rings and small logo responses. Include
  the transparent 8.4-second looping SVG/PWA copy, interactive preview, GIF and editable
  static SVG/AI artwork. Chrome checked the loop seam, all signal paths, controls, light/dark
  widths 320–768 and reduced motion; VectorCraft reopened the AI without warnings. An isolated
  headless kimo served the matching SVG. Clippy, 130 tests and the headless build passed;
  formatting check reports unrelated file-browser Rust edits (details in the brand README). Actual
  pairing/routing, Safari and real-device acceptance remain separate checks.

## Native app (Rust)

- [x] Tabs per project with a + menu (terminal, Claude, Codex); the projects panel hides (⌘B); drops on the panel add projects, anywhere else type paths.
- [x] kimo.app in Applications (Dock, Launchpad), kept at the newest version with ~/.local/bin/kimo; the login shell's PATH when opened from the Dock.
- [ ] Show the window's views in its own menu bar (macOS) and remember the window size.
- [x] Bottom bar: plan use of the active subscriptions, keep awake (on / while agents work / off), resources per project, ports by terminal, kimos; the same as chips in the PWA (verified in Chrome on desktop width; not on a real phone).
- [ ] Open the bottom-bar panels in the headless window test (the bar itself is checked).
- [x] System clipboard in the window (text both ways, images: Ctrl+V to local Claude/Codex, upload + path elsewhere); session right-click menu (open, stop, remove); loose sessions by folder with + add as project; rounded Dock icon while running.
- [ ] Verify image paste with real Claude Code and Codex in the window (not done: no screen access here).

- [x] `kimo` opens a native window (egui) that is a client of the local server (started in the background when needed).
- [x] Terminals in the window on alacritty_terminal: colours, keys, Tab, Ctrl+C, paste, scrollback, resize, terminal replies.
- [x] Projects sidebar with drag-and-drop of folders, + claude / + codex / + shell per project.
- [x] Subscriptions, Skills, Settings and Devices (QR pairing) views.
- [x] Text selection and copy in the terminal (drag, double-click, Ctrl+C with a selection, Ctrl+Shift+C, right-click).
- [x] Mouse wheel for full-screen programs (SGR / X10 wheel reports, arrows in the alternate screen).
- [ ] Mouse reporting of clicks and drags for TUIs that use the mouse.
- [x] Dark mode (Settings → Appearance, window and web client).
- [x] Resizing settles before the agent is told; *redraw* reloads a garbled terminal.
- [x] Drop files on the terminal to type their paths (macOS reads the drop position from CoreGraphics; Linux uses the last pointer position).
- [ ] Drop position on Linux (X11/Wayland report no pointer moves while dragging).
- [x] Update check and one-click update with restart (agents continue).
- [ ] Signed release builds for macOS (notarized app bundle) so updates and first runs need no Gatekeeper prompt.
- [x] Symbol fallback fonts (DejaVu Sans Mono, Noto Sans Symbols 2) so agent icons and box drawing render.
- [ ] Emoji font, and the few symbols still missing (⎿).
- [ ] Split panes (several terminals side by side) and keyboard shortcuts to switch tabs (⌘T opens one in the project).
- [ ] Native folder dialog for "+ project" (xdg portal) besides the server folder picker.
- [x] App icon (the kimo logo) and a .desktop launcher (`contrib/kimo.desktop`).
- [ ] `kimo install` (service + launcher + icon in one command).
- [ ] Lower the window's idle cost: repaint only when shown values change (usage updates arrive every 2 s), measure on a GPU desktop.
- [ ] Desktop notifications when an agent needs you or finishes.
- [x] Update notifications (system notification, Dock bounce, card, top-bar button, phone banner) and updating by itself when idle.
- [x] Restart a kimo server older than the window (including servers before 0.1.2) in one click.
- [ ] Click a system notification to open kimo (osascript notifications open Script Editor; needs a notifying helper inside kimo.app).
- [x] Host switcher: every paired kimo in one window (projects, sessions, terminals, settings).

## Core Terminal Management

- [x] Implement a robust terminal emulator within the ADE. (PTY per session + xterm.js in the PWA, scrollback replay on reconnect)
- [x] Allow users to create, manage, and switch between multiple terminal sessions.
- [x] Develop a system for pausing and resuming terminal sessions. (pause = SIGSTOP, stop = free memory, continue = `--continue` / `resume --last`)
- [x] Integrate with Tailscale or Zerotier for secure remote access. (`--bind tailscale|zerotier|lan|ip` + token)
- [x] Agents survive kimo updates: the PTYs are handed to the new kimo across the exec (same processes, screens, paused state); restart-and-continue as the fallback.
- [ ] Detachable session holder so interactive agents also survive a kimo crash (separate tiny PTY daemon). Jobs (`/api/jobs`) already survive crashes.
- [x] Pin each Claude session to its own conversation (`--session-id` / `--resume <id>`); fresh start when the transcript does not exist yet.
- [x] Do not leak kimo's token or a parent agent's `CLAUDE_CODE_*` environment into sessions.
- [x] Pin Codex sessions to their conversation (id read from the Codex log after start; `codex resume <id>`).
- [x] Clear ids: kimo id and resume id shown in the window, the web client and `kimo agents`.
- [ ] Rename sessions, edit args/folder, reorder or pin them.
- [x] Projects: new folder (+ git init), existing folder, clone in a visible terminal; branch and agents per project.
- [x] Adding a folder continues its earlier Claude and Codex conversations; project history with continue (window, phone, `kimo history`/`continue`).
- [x] Usage tab: tokens per day, project (or repository) and subscription for every agent conversation on the machine (window, phone, `kimo usage`, API).
- [x] Folder picker on the server for new sessions and projects.
- [ ] Project templates (start a project from a starter folder) and per-project default agent/account.
- [x] Worktrees: create a git worktree per agent from a project (window + / phone, API, SDK); remove it when clean.
- [x] Projects like Orca: collapse, ··· menu, project settings (name, colour), open the latest agent.
- [ ] Worktree base branch choice and "create PR" from a worktree.

## Account & Subscription Management

- [x] One active subscription per provider (new agents start on it; switching can move the agents), this computer's own login named by its email.
- [x] Cost per model at API prices in Usage (window, phone, API).

- [x] Enable definition and management of multiple Claude and Codex accounts. (one config home per account, signed-in status)
- [x] Implement a mechanism for seamless switching between subscriptions. (per-session account select, restarts and continues)
- [x] Allow users to easily change subscriptions without agent logout. (Accounts → move sessions, shared conversations via links)
- [x] Usage per subscription: runs per account; Claude tokens (5 h / 7 days) from transcripts; Codex plan limits and reset times from its logs.
- [x] Claude plan percentages (Claude's usage API, as Claude Code's /usage shows them), with the pace of every plan limit.
- [ ] Refresh an expired Claude token for the plan limits instead of waiting for Claude to run on that login.
- [ ] Handle subscription limits: detect "limit reached" (output / Codex `rate_limit_reached_type`) and offer or perform the move to another account automatically.
- [ ] Phone-friendly Codex sign-in (device code flow or token import).

## Resource Monitoring & Optimization

- [x] Track resource usage (CPU, RAM, network) of each terminal. (RAM, CPU, process count per process tree; network still missing)
- [ ] Per-session network usage (cgroup or /proc/<pid>/net is per-namespace: needs cgroups v2).
- [x] Implement resource limits to prevent agent saturation: when memory is critical, idle Claude/Codex agents are stopped (biggest first) and continue when opened; low-memory card with "stop idle agents". (Settings → Memory)
- [ ] Optional per-session cgroup memory.max (Linux).
- [x] Auto-stop sessions idle for N minutes (Settings → Memory → stop agents idle for more than).
- [x] Memory breakdown per agent (the agent, each MCP server or tool it started).
- [ ] Share MCP servers between agents of a project (each Claude Code starts its own copies: often 30-50 % of an agent's memory).
- [x] Provide visualizations of resource consumption. (stat tiles, per-session memory bar, low-memory alert)
- [ ] History chart of memory/CPU per session.
- [x] Optimize the Rust codebase for minimal resource footprint. (3.5 MB binary, ~5 MB RSS, two small threads per running session)

## Mobile-First Design & Integration

- [x] Installable app: typed pairing codes for an installed app (iPhone keeps its own storage), install prompt on Android over HTTPS, a switch between paired kimos.

- [x] Design the ADE with a mobile-first approach. (phone layout, bottom tabs, key bar, compose box)
- [x] Explore PWA (Progressive Web App) integration for accessibility. (manifest, service worker on HTTPS via `tailscale serve`)
- [x] Develop a mobile control interface for managing agents.
- [x] Photos for agents from the phone camera or gallery (uploads, thumbnails, paths in the message).
- [ ] Paste or drop images into the window's message box (same upload).
- [x] Ensure seamless integration with Tailscale or Zerotier for mobile access.
- [ ] Push notifications when an agent waits for input, finishes or hits a limit (Web Push from the server).
- [x] "Needs you": sessions waiting for an answer or approval, first in the phone list and a button in the window.
- [x] Event stream (SSE) instead of 2 s polling (`/api/events`, used by kimo's own PWA).

## Platform for other PWAs

- [x] Per-app keys bound to origins and a scope (read / control), revocable, managed in the Apps tab.
- [x] CORS for listed origins; cross-site requests always need a key (closes the localhost-without-token hole).
- [x] JS SDK served by kimo (`/sdk/kimo.js`) and an embeddable `<kimo-terminal>` element; kimo's own PWA uses both.
- [x] `last_output_at` per session so apps can show "quiet for".
- [x] Example app on another origin (`examples/agent-board`).
- [ ] Example integrations: a project manager that links tasks to agent sessions.
- [ ] Richer events: per-session `waiting for input`, `finished`, `limit reached` (parsed from output) for app notifications.
- [ ] Webhooks for server-side apps (POST events to a URL) in addition to SSE.
- [ ] Per-key limits (which folders/sessions an app may touch) and an audit log of app actions.
- [ ] TypeScript types for the SDK; publish it as a package as well.
- [x] Systemd user unit (`contrib/kimo.service`).
- [x] Use kimo on the server itself without logging in (the machine is the owner).
- [x] Pair phones and other devices with a QR code (Devices tab, `kimo pair`); unpair anytime.
- [x] Show every address kimo is reachable on (LAN, Tailscale IP/HTTPS, ZeroTier); listen on all networks by default.
- [x] One-tap HTTPS on the tailnet (`tailscale serve` on a free port) so phones can install the app.
- [ ] `kimo install` command (writes the unit, enables lingering) instead of manual steps.
- [ ] Show the pairing QR on the server's terminal at first start when no device is paired.
- [ ] mDNS (`kimo.local`) so LAN devices find kimo without an IP.

## Skills and settings (like Orca)

- [x] Skills: list global and per-project skills for Claude, Codex and `.agents`; install from GitHub/git/folder; remove to backup; synced/built-in read-only.
- [x] Settings: per-agent permissions (manual / yolo), model, extra args, enabled; terminal font size; quick commands.
- [x] Detect installed agent CLIs and versions.
- [ ] Skills and settings in the phone/web client too (the API is ready).
- [ ] Per-project settings (default agent, account, model) and project-scoped quick commands in the UI.
- [ ] Skill updates: show when an installed skill's source has a newer version.
- [ ] MCP servers per agent (list, add, remove).

## Agents working together and several machines

- [x] `kimo agents | send | read | wait | new | whoami`; signed messages; last answer from the agent's own transcript.
- [x] Bundled `kimo-agents` skill (one-click install) that teaches agents to use them.
- [x] Pair kimos with each other (`kimo peer add`, Devices → Other kimos); agents of every machine as `host/id`; messages, reads and actions routed across machines.
- [x] Remote terminals in the phone client (`/api/hosts/{host}/…` proxies calls, events and terminals through the local kimo).
- [ ] HTTPS between kimos (TLS in the client) and SSH as a built-in transport.
- [ ] Group chat / shared channel for a team of agents on a task.
- [x] Map tab: every machine and phone on the tailnet in 3D, to scale, linked to this kimo (`GET /api/network/tailnet`, SDK `tailnet()`); each machine's projects and agents (its kimo's `/projects` + sessions), facts (`/node`) and fleet state/jobs; pulses when work starts/ends; All, Top (2D) and Labels controls. Verified in Chrome on a dev kimo with the real tailnet (desktop width); not yet on a phone, with paired kimos, or with a fleet coordinator.
- [x] Map demo data: `web/js/map-demo.js` is a stand-in for the SDK calls the map makes over the invented world of `scripts/demo/world.mjs` (the one the screenshots show: agents starting and finishing on every poll, a training job, wake), so `initMap({ api: demoApi(world) })` runs without a kimo. Checked through the map's data layer in Node; shown live on the website.
- [x] Map and fleet together: hardware models and roles from the coordinator's registry pick the 3D model and the role label; tailnet machines not in the fleet get *add to the fleet*, which opens the Fleet tab's finder (`tests/js/map-data.mjs`). Seen at phone width in the screenshots (`phone-map.png`).
- [x] Demo kit (`scripts/demo/`): a real kimo with an isolated HOME, invented projects, terminals, accounts, usage logs and history, behind a proxy that answers what cannot be real on one machine (the tailnet, paired kimos, the fleet) from `world.mjs`. `--capture` takes the phone screenshots (headless Chrome) and the window's (`KIMO_CAPTURE`), light and dark. Nothing in them is real.
- [ ] Map: show which machine runs each fleet job as a moving pulse from the coordinator; check it on a real phone with paired kimos and a coordinator.

## Fleet: several machines, power, jobs and workflows (2026-10-28)

Built from the owner's fleet evaluation (Phases 1–4); guide in [docs/fleet.md](docs/fleet.md).

- [x] One-way pairing for coordinators (`/api/pair` with `role: fleet`): fleet keys reach only `/status`, `/node`, `/jobs`, `/power`; listed and revocable in Devices; never from a web page. Kept in a separate state list so older kimos never treat them as owner keys.
- [x] `GET /api/node`: hardware, NVIDIA GPUs/driver/CUDA, Jetson L4T, Raspberry Pi model, Hailo, Xcode, load, disk, logins, sessions/jobs, `in_use`, labels.
- [x] `GET /api/power`, `POST /api/power/shutdown`: refused while terminals, jobs or logins keep the machine on (409 with blockers); only the owner forces it; `KIMO_SHUTDOWN_COMMAND`, `KIMO_SHUTDOWN_IGNORE_LOGINS`.
- [x] Jobs on every kimo: detached `kimo job-run`, file-backed state (survives updates and crashes), log, timeout, cancel, exit code, headless `claude -p` / `codex exec` with the answer as result, staged inputs, output files, fresh git worktree or clone per job; running jobs keep the machine awake and on.
- [x] Peers that did not answer are skipped for 20 s instead of slowing every listing.
- [x] `kimo-fleet` crate: `devices.yaml` registry, Wake-on-LAN, readiness (facts, GPU visible), power states (off/waking/booted/ready/busy/idle/draining/unreachable/manual), wake retry, boots per day, idle shutdown through the machine's own check, holds and manual control; scheduler (labels, VRAM, memory, exclusive GPU/slots, up before off, cheapest first, prefer/pin, waiting with a reason); retries on the same machine, failover after a lost machine; workflows (needs, optional steps, skipped steps, approvals), artifacts by SHA-256 between steps; SQLite events/samples/states/jobs/workflows; ntfy; CLI; looks, dispatches and follows on their own threads so a silent machine blocks nothing.
- [x] `/api/fleet` proxy in kimo (`KIMO_FLEET`), SDK (`jobs`, `node`, `power`, `kimo.fleet.*`), Fleet tab on the phone (machines, jobs with logs/outputs/approvals, workflows, events, new job).
- [x] Release builds and the install script include `kimo-fleet`; CI tests the workspace; `contrib/kimo-fleet.service`.
- Verified: unit tests, `tests/api.rs` (keys, power, jobs, checkouts), `fleet/tests/fleet.rs` with real kimos on local ports (wake packet caught locally, job on the woken machine, output kept, idle shutdown, holds, a workflow across three machines with retry, optional failure, stand-in Claude and approval), SDK script; the Fleet tab in Chrome at phone width against local demo servers (approve, log, a job queued from the phone).
- [ ] Real hardware: Wake-on-LAN from S5 on both NUCs, `systemctl poweroff` under a user service (sudoers rule), `nvidia-smi`/Jetson/Hailo facts, kimo-fleet on the Pi 4 (ARM build, SSD), ZeroTier addresses.
- [ ] Real providers in jobs: `claude -p` and `codex exec` with real accounts and `permissions: yolo`.
- [ ] The Fleet tab on a real phone.
- [ ] Web Push for fleet notices (ntfy works today); per-key folder limits for fleet jobs; scheduled backups of `fleet.db`; registry editing from the phone (the API has `PUT /registry`).
- [ ] Paired kimos are still owners of each other: pair the phone only with the coordinator's kimo; a scoped peer key is a follow-up.

## Release

- [x] License MIT OR Apache-2.0, third-party notices; own web stylesheet (no commercial theme).
- [x] CI (Linux and macOS) and release builds (Linux x86_64/arm64, macOS arm64/x86_64), install script.
- [x] macOS support (compile-checked and tested in CI): ps/sysctl/vm_stat stats, ifconfig, LaunchAgent.
- [ ] Run on a real Mac and a Raspberry Pi; Wayland and a GPU desktop for the window.
- [ ] Publish the clean public repository and tag v0.1.0.

## Ideas from herdr (herdrdev/herdr)

- [x] Agent state per session (working, waiting, idle) from output timing and approval prompts; in events, the SDK (`waitFor`) and the CLI (`kimo wait`).
- [ ] Use the agents' own hooks (Claude Code hooks, Codex `notify`) for exact state, and a `done` state.
- [x] Orchestration from apps and agents: `kimo new --prompt`, `send`, `wait`, `read`; SDK `send`, `waitFor`, `reply`.
- [ ] Plugins: a folder with a `kimo-plugin.toml` manifest; event hooks (session.blocked, session.done, project.created…), actions, and an optional web UI that kimo serves under `/plugins/<id>/` with its own app key, so a plugin can be a full PWA hosted by kimo. Install from GitHub, review before enabling.
- [ ] More agents: OpenCode, Cursor, Copilot, Gemini… (launch + resume command per agent).
