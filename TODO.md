# TODO

## Native app (Rust)

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
- [ ] Split panes (several terminals side by side) and keyboard shortcuts to switch tabs.
- [ ] Native folder dialog for "+ project" (xdg portal) besides the server folder picker.
- [x] App icon (the kimo logo) and a .desktop launcher (`contrib/kimo.desktop`).
- [ ] `kimo install` (service + launcher + icon in one command).
- [ ] Lower the window's idle cost: repaint only when shown values change (usage updates arrive every 2 s), measure on a GPU desktop.
- [ ] Desktop notifications when an agent needs you or finishes.
- [x] Host switcher: every paired kimo in one window (projects, sessions, terminals, settings).

## Core Terminal Management

- [x] Implement a robust terminal emulator within the ADE. (PTY per session + xterm.js in the PWA, scrollback replay on reconnect)
- [x] Allow users to create, manage, and switch between multiple terminal sessions.
- [x] Develop a system for pausing and resuming terminal sessions. (pause = SIGSTOP, stop = free memory, continue = `--continue` / `resume --last`)
- [x] Integrate with Tailscale or Zerotier for secure remote access. (`--bind tailscale|zerotier|lan|ip` + token)
- [ ] Detachable session holder so agents survive a kimo restart/upgrade (separate tiny PTY daemon or re-attach via tmux).
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

- [x] Enable definition and management of multiple Claude and Codex accounts. (one config home per account, signed-in status)
- [x] Implement a mechanism for seamless switching between subscriptions. (per-session account select, restarts and continues)
- [x] Allow users to easily change subscriptions without agent logout. (Accounts → move sessions, shared conversations via links)
- [x] Usage per subscription: runs per account; Claude tokens (5 h / 7 days) from transcripts; Codex plan limits and reset times from its logs.
- [ ] Claude plan percentages, if Anthropic exposes them in a supported way.
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
- [ ] Remote terminals in the phone client (proxy the terminal websocket through the local kimo).
- [ ] HTTPS between kimos (TLS in the client) and SSH as a built-in transport.
- [ ] Group chat / shared channel for a team of agents on a task.

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
