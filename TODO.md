# TODO

## Native app (Rust)

- [x] `kimo` opens a native window (egui) that is a client of the local server (started in the background when needed).
- [x] Terminals in the window on alacritty_terminal: colours, keys, Tab, Ctrl+C, paste, scrollback, resize, terminal replies.
- [x] Projects sidebar with drag-and-drop of folders, + claude / + codex / + shell per project.
- [x] Subscriptions, Skills, Settings and Devices (QR pairing) views.
- [ ] Text selection and copy in the terminal; mouse reporting for TUIs that use the mouse.
- [ ] Symbol/emoji fallback font so agent icons (✓ ⏵ ✻) render.
- [ ] Split panes (several terminals side by side) and keyboard shortcuts to switch tabs.
- [ ] Native folder dialog for "+ project" (xdg portal) besides the server folder picker.
- [ ] App icon and .desktop file; `kimo install` (service + launcher).
- [ ] Lower the window's idle cost: repaint only when shown values change (usage updates arrive every 2 s), measure on a GPU desktop.
- [ ] Notifications from the window (agent waiting / done) once agent state exists.

## Core Terminal Management

- [x] Implement a robust terminal emulator within the ADE. (PTY per session + xterm.js in the PWA, scrollback replay on reconnect)
- [x] Allow users to create, manage, and switch between multiple terminal sessions.
- [x] Develop a system for pausing and resuming terminal sessions. (pause = SIGSTOP, stop = free memory, continue = `--continue` / `resume --last`)
- [x] Integrate with Tailscale or Zerotier for secure remote access. (`--bind tailscale|zerotier|lan|ip` + token)
- [ ] Detachable session holder so agents survive a kimo restart/upgrade (separate tiny PTY daemon or re-attach via tmux).
- [x] Pin each Claude session to its own conversation (`--session-id` / `--resume <id>`); fresh start when the transcript does not exist yet.
- [x] Do not leak kimo's token or a parent agent's `CLAUDE_CODE_*` environment into sessions.
- [ ] Pin Codex sessions to their conversation (read the id from `~/.codex/sessions` after start) instead of `resume --last`.
- [ ] Rename sessions, edit args/folder, reorder or pin them.
- [x] Projects: new folder (+ git init), existing folder, clone in a visible terminal; branch and agents per project.
- [x] Folder picker on the server for new sessions and projects.
- [ ] Project templates (start a project from a starter folder) and per-project default agent/account.
- [ ] Worktrees: create a git worktree per agent from a project.

## Account & Subscription Management

- [x] Enable definition and management of multiple Claude and Codex accounts. (one config home per account, signed-in status)
- [x] Implement a mechanism for seamless switching between subscriptions. (per-session account select, restarts and continues)
- [x] Allow users to easily change subscriptions without agent logout. (Accounts → move sessions, shared conversations via links)
- [ ] Usage per subscription: record which account each session ran under (and when), then
      - Codex: read `rate_limits` from its session logs (used %, window, reset time are there);
      - Claude: sum tokens per account from the transcripts (`usage` per message); plan % only if Anthropic exposes it.
- [ ] Handle subscription limits: detect "limit reached" (output / Codex `rate_limit_reached_type`) and offer or perform the move to another account automatically.
- [ ] Phone-friendly Codex sign-in (device code flow or token import).

## Resource Monitoring & Optimization

- [x] Track resource usage (CPU, RAM, network) of each terminal. (RAM, CPU, process count per process tree; network still missing)
- [ ] Per-session network usage (cgroup or /proc/<pid>/net is per-namespace: needs cgroups v2).
- [ ] Implement resource limits to prevent agent saturation: memory budget that auto-stops the longest-idle agents, optional per-session cgroup memory.max.
- [ ] Auto-pause/stop sessions idle for N minutes (no output, no input).
- [x] Provide visualizations of resource consumption. (stat tiles, per-session memory bar, low-memory alert)
- [ ] History chart of memory/CPU per session.
- [x] Optimize the Rust codebase for minimal resource footprint. (3.5 MB binary, ~5 MB RSS, two small threads per running session)

## Mobile-First Design & Integration

- [x] Design the ADE with a mobile-first approach. (phone layout, bottom tabs, key bar, compose box)
- [x] Explore PWA (Progressive Web App) integration for accessibility. (manifest, service worker on HTTPS via `tailscale serve`)
- [x] Develop a mobile control interface for managing agents.
- [x] Ensure seamless integration with Tailscale or Zerotier for mobile access.
- [ ] Push notifications when an agent waits for input, finishes or hits a limit (Web Push from the server).
- [ ] "Needs attention" view: sessions whose last output is a question/permission prompt, with quick replies via `/input`.
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

## Ideas from herdr (herdrdev/herdr)

- [ ] Agent state per session: `working`, `blocked` (needs an answer/approval), `done`, `idle`. Prefer the agents' own hooks (Claude Code hooks, Codex `notify`) reporting to kimo over reading the screen; expose it in events and the SDK (`kimo.wait(id, 'idle')`).
- [ ] "Needs you" list and notifications driven by that state (the most useful thing for a phone).
- [ ] One-call orchestration for apps: create project + start agent + first prompt; `prompt --wait`.
- [ ] Plugins: a folder with a `kimo-plugin.toml` manifest; event hooks (session.blocked, session.done, project.created…), actions, and an optional web UI that kimo serves under `/plugins/<id>/` with its own app key, so a plugin can be a full PWA hosted by kimo. Install from GitHub, review before enabling.
- [ ] More agents: OpenCode, Cursor, Copilot, Gemini… (launch + resume command per agent).
