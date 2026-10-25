# Changelog

## 0.1.0 (unreleased)

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
