# TODO

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
- [ ] Folder picker / recent git worktrees on the server for the "new session" sheet.

## Account & Subscription Management

- [x] Enable definition and management of multiple Claude and Codex accounts. (one config home per account, signed-in status)
- [x] Implement a mechanism for seamless switching between subscriptions. (per-session account select, restarts and continues)
- [x] Allow users to easily change subscriptions without agent logout. (Accounts → move sessions, shared conversations via links)
- [ ] Handle subscription limits and automatic updates: detect "limit reached" in the output and offer/perform the move automatically.
- [ ] Show usage/limit windows per account (parse `/usage` / `codex` status) and reset times.
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
- [ ] Event stream (SSE) instead of 2 s polling.
- [ ] Systemd user unit + install script.
