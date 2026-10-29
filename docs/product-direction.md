# Product direction

Owner's direction, recorded 2026-10-26. This guides implementation, reviews and prioritisation;
it describes the intended product, not a claim that every scenario already passes.

## Purpose

kimo is a mobile-first agent development environment (ADE). From a phone or tablet, a user
connects through a PWA to a computer they own, creates a project from nothing, and directs
coding agents that use that computer's CPU, RAM, storage and GPU when applicable. The agents
can build the project and publish to GitHub or another destination when the user asks.

The host might be a Mac mini, MacBook Air, NUC with a GPU, or another supported macOS or Linux
machine. Setup and daily use should be simple enough that the user can work remotely without
returning to a desktop window. Host and provider prerequisites should be explained during
setup, and remaining steps that need physical access must be visible as limitations.

## Principles

1. **Mobile first, including starting from zero.** Creating a fresh folder and project,
   selecting an agent and account, giving it a task, answering it and reviewing progress are
   primary PWA workflows. Existing repositories and desktop access are useful options.
   Design for touch, small screens and reconnecting after the phone sleeps or changes network.
2. **Compute stays on the host.** Agents, terminals, builds and files live on the selected
   computer. Losing a client connection must not stop work. Headless use must remain possible
   without the native UI or desktop dependencies.
3. **Kimos work together.** Pair machines once, then reach their projects and agents from a
   phone or another kimo. A MacBook can continue a conversation and control a project on a NUC
   without copying that project to the MacBook. Keep host identity clear and route creation,
   terminals, messages, events and actions to the selected host.
4. **Rust efficiency matters in practice.** Keep the core and its dependencies small. Multiple
   terminals and agents must not create disproportionate kimo overhead. Show host and
   per-agent memory use, warn under pressure and suggest useful actions. Pausing a process
   leaves its RAM allocated; stopping a resumable agent can free RAM and continue its
   conversation later. Make the action and its effects clear. Preserve configurable recovery
   for idle agents and avoid interrupting active work as a routine response.
5. **Accounts can change without losing work.** With multiple authenticated accounts for
   Claude Code or Codex, moving a session to another account must continue that session's own
   conversation. Avoid logout/login cycles and manually reopening each chat. Make usage and
   exhausted limits visible; offer another suitable account when limits are reached. Treat
   initial account authentication separately from switching accounts that are already ready.
6. **The core supports custom experiences.** A shared API, live events, SDK and terminal
   component let users build their own mobile PWAs and manage cooperating agent teams on one
   or several hosts. kimo's PWA uses that same interface. Respect app scopes and owner-only
   controls; richer team views can be built on top.
7. **Keep the environment simple.** Projects remain ordinary folders with ordinary tools.
   Keep internal state in kimo's data directory and credentials in account homes. A file
   explorer, editor or elaborate project-management framework is not required for the core
   experience. Add core features when they make remote creation, control, continuity or
   extensibility work better; use custom apps for specialised interfaces.

## Acceptance scenarios

Use these when verifying changes to the relevant workflow. Record the host, client, account
provider and result; distinguish simulated integration tests from actual phone/provider use.

| Scenario | Required outcome |
| --- | --- |
| Start from a phone | With a configured host and authenticated account, pair a phone PWA, select that host, create a project that did not exist, launch the chosen agent in its new folder and send the initial task. No desktop interaction is needed. |
| Deliver remotely | From the PWA, direct the agent to build and, when requested, publish using the host's authorised tools. Progress, failures and any required user input remain accessible on the phone. |
| Reconnect | Lock or disconnect the phone while work runs; return to the same project and conversation with current output. The client disconnect does not terminate the agent. |
| Continue on another device | Create work on a NUC, then use a paired MacBook or tablet to continue that conversation on the NUC. Show the host clearly; do not silently create local copies or sessions. |
| Control several hosts | Through one paired kimo, create and control projects and agents on another; forward terminal traffic, live events and messages correctly. An unreachable peer must leave local work usable. |
| Change accounts | Switch running Claude Code and Codex sessions between already authenticated accounts of the same provider. Preserve each conversation ID and history without logging out or asking the user to resume chats by hand. |
| Recover from a limit | Detect an exhausted account and offer a suitable account switch from the phone. Continue the affected conversation after the switch; keep automatic switching a user-controlled policy. |
| Recover memory | Run several agents; show host and per-agent memory pressure and a mobile recovery action. Stopping an idle resumable agent releases its processes and opening it continues the same conversation. Describe pause accurately. |
| Build on kimo | A separate app with an appropriate scoped key can create a project, start and message agents, follow their state, read replies and show a terminal through the public SDK. Owner-only operations remain protected. |
| Stay light and headless | The server builds without desktop features. Measure idle server memory and the added overhead of several sessions separately from agent/tool memory; include machine and build details with performance claims. |

## Source review and remaining work

Reviewed 2026-10-26 against the current sources. This is a source review, not a completed
real-device acceptance run. Existing checked tasks in `TODO.md` remain implementation history.

| Area | Evidence already in the repository | Follow-up |
| --- | --- | --- |
| New remote projects | `src/services/projects.rs`, `web/js/projects.js`, SDK `createProject`, and `tests/api.rs` cover folder/project creation. The PWA can then create an agent and send a task. | The fresh-project form has no agent/account/initial-task fields; launching and instructing the agent take separate steps. Simplify this journey and verify it from a real phone on the chosen host. |
| Several hosts | `src/services/peers.rs`, `src/api/hosts.rs`, SDK host selection and `tests/peers.rs` provide pairing, routing and cross-host messaging. | Verify project creation, terminal reconnects and continuation through a peer from the PWA. TLS is disabled in the Rust HTTP/WebSocket client dependency features in `Cargo.toml`; HTTPS/WSS peer access remains work. |
| Accounts and limits | `src/services/accounts.rs`, `set_account` in `src/services/sessions.rs`, the SDK and `tests/sessions.rs` support isolated account homes and conversation-preserving switching. | Existing PTY account-switch coverage uses a fake Claude CLI; Codex resume is tested separately. Verify switching with both real providers. Limit-triggered recovery and phone-friendly Codex authentication remain open tasks. |
| Memory | `src/core/memory.rs`, `src/services/memory.rs`, the PWA warning/actions and `tests/sessions.rs` implement pressure reporting, idle-agent stopping and continuation. | Verify the mobile recovery flow and measure overhead on representative hosts. Keep pause and memory-freeing stop distinct in copy. |
| Custom apps | `web/sdk/`, `examples/agent-board`, and scope/origin tests in `tests/api.rs` provide an app foundation. | Verify the creation-to-agent workflow through an independent client; evolve team orchestration through shared interfaces. |
| Mobile completeness | `TODO.md` already records PWA support, typed pairing and a host switcher. | Finish mobile skills/settings controls and account setup; test sleep/reconnect on iOS and Android. Desktop UI completion alone does not close these tasks. |

## Priorities and review

Prioritise gaps in the complete phone-to-host workflow, continuity across devices and
accounts, resource recovery, and the shared API/SDK. Desktop refinements and features
inspired by Orca or herdr follow these goals. Competitor parity is not a requirement by itself.

For a relevant change, report which acceptance scenario it improves, where it is available
(API, SDK, PWA, native client), what was actually verified, and any remaining host-only step
or limitation. Do not mark a phone journey complete solely because its endpoint exists.
Do not make comparative claims about competitors or unmeasured resource claims without evidence.
