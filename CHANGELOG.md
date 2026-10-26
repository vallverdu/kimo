# Changelog

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
