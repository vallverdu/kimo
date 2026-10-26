# Changelog

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
