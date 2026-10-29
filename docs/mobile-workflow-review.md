# Mobile workflow review: lessons from Herdr

Reviewed 2026-10-26 against kimo `a3611de` / v0.1.11. These are recommendations for the
main implementation agent, aligned with [product direction](product-direction.md) and
the priorities at the top of [TODO.md](../TODO.md). They are not implemented features.

## Evidence and product position

Reviewed Herdr's current documentation and the independent **wherdr** mobile companion,
including its public demo in Chrome with a 390 × 844 touch viewport. Opened its agent list,
conversation and new-agent form. The demo identified itself as wherdr 2.1.0 and uses scripted
data. This was browser emulation, not a real phone, provider run or installed Herdr test.

In the demo, a conversation showed readable prose, a folded tool group, an inline question
and a composer; the new-agent form combined agent choice, folder and initial message.
These are useful references for completing a task from a phone.
Source: [wherdr public demo](https://wherdr.dev/demo/).

Herdr documents multiple machines with independent connections and explicit stale state.
Its documentation also distinguishes detach, process restart, transcript replay, native
conversation resume and experimental live process handoff. These are different guarantees.
Sources: [machines](https://herdr.dev/docs/connecting-machines/),
[session state](https://herdr.dev/docs/session-state/).

kimo's opportunity is a coherent, small server with a shared HTTP API and SDK, ordinary
project folders and a complete phone-to-host creation and delivery workflow. Mobile access,
multiple hosts and resumable conversations alone do not establish superiority over other
products. Rust alone does not establish lower resource use. Measure comparable workloads
before making performance claims. Rich task boards can remain custom apps on the SDK.

## Recommended order

### 0. Close trust and routing gaps

The private audit is at `.git/kimo-audit-2026-10-26.md`; it contains reproductions and limits.
Revalidate it against the current release before implementing fixes. Prioritise owner access
validation, revocation of peer credentials and existing sockets, and private state creation.
Also fix cross-origin SDK methods and complete HTTPS/WSS peer support already in TODO.
Do not treat removing a host from a list as proof that its access has been revoked.

There is an additional reproduced SDK routing bug in `web/sdk/kimo.js`: `send()` performs
two requests separated by awaits, each consulting mutable `this.host`. Calling `useHost(B)`
after beginning a send to A sends text to A and Enter to B. `sendFile()` also consults the
host after awaiting `file.arrayBuffer()`. A mocked-fetch reproduction confirmed both URL
changes; no real session received these requests. The usual outcome is failure on the
wrong host; an existing matching session identifier could receive the request instead.

Capture an immutable host client at the start of every multi-step action and upload. The
existing `forHost()` can support this. In-flight responses must not populate a newly selected
host's view. Bind drafts and attachment results to host plus conversation/session identity.
Keep the chosen host visible in creation, review and notification destinations.

### 1. Make creating and directing a project one phone journey

Extend the existing project-creation flow with agent, authenticated account and initial task.
Show the target host and folder before launch. Preserve existing-folder and clone choices;
a clone must finish successfully before starting the requested agent. Show which steps
succeeded and let the user resume a failed launch without creating duplicate folders or agents.

Implement shared behaviour through services, API and SDK, then the PWA. Reuse existing
project/session/account mechanisms. A desktop-only form does not complete this work.

Acceptance: on a real phone, create a previously absent folder on a selected remote host,
launch the chosen provider/account, send a task and direct delivery without opening a native
window. Record host, client, provider and any remaining host-only authentication step.

### 2. Add a readable conversation view alongside the terminal

wherdr documents transcript rendering with grouped tools, search and images. Passive reading
does not resize the agent's terminal. Its terminal view separately takes control of sizing
and restores the prior size when released.
Sources: [conversation](https://github.com/afloury/wherdr/blob/main/docs/conversation.md),
[terminal](https://github.com/afloury/wherdr/blob/main/docs/terminal.md).

kimo already parses provider replies in `src/core/replies.rs` and reads transcripts in
`src/services/talk.rs`; `/reply` returns only the latest answer. The PWA in
`web/js/terminal.js` opens a live terminal. Extend the shared interface with bounded,
cursor-based conversation history and incremental updates, scoped to the session's known
conversation and account. Avoid rescanning entire logs for every client poll. Preserve
unknown provider events and offer terminal access when a provider format is unsupported.

Render Markdown safely, group tool details and retain normal text selection. Reading must
not attach a control socket or resize a PTY. Explicit terminal takeover needs a bounded
size lease that expires on disconnect. Preserve draft text and visibly report sends that
failed or have uncertain delivery; do not clear the composer merely because bytes were queued.

Acceptance: read a long Claude and Codex conversation on a phone, retain scroll position
through updates, and verify that reading leaves a simultaneously attached desktop's PTY
dimensions unchanged. Exercise read and control app scopes through an independent SDK client.

### 3. Make waiting and delivery visible

wherdr documents completion/question push notifications, quick replies and per-device quiet
settings. These are references for reducing repeated phone checks, rather than evidence of
kimo support. Source: [wherdr features](https://wherdr.dev/#features).

kimo's `src/core/activity.rs` currently infers waiting from output timing and phrases. Keep
such heuristics labelled as uncertain. Prefer provider hooks or structured events for
questions and approvals. Render action buttons only for a current, identified interaction;
include the host, session and interaction revision when answering, reject stale answers,
and never infer permission to approve from ordinary transcript text.

Start with an attention list and reliable send/error states. Then add opt-in Web Push using
the shared event model, per-device preferences and links to the original host/conversation.
Revoke subscriptions with device access, deduplicate events and avoid putting sensitive
question text on a lock screen by default. Notifications must not keep a terminal attached.

Acceptance: a real iOS/Android installed PWA receives a notification after backgrounding;
opening it returns to the same host/conversation. A question answered on another client
cannot be submitted again from an old notification. Reconnection does not duplicate a send.

### 4. Recover visibly from interrupted connections and accounts

Show online, reconnecting, unreachable and authentication-required states with last-update
time. Cached information must be visibly stale and must not accept terminal input. A failed
peer should leave local and other available work usable. Capture operation destinations
before awaits, reject old host responses and perform a fresh snapshot/cursor catch-up after
phone sleep. Do not automatically repeat a mutation whose outcome is unknown.

kimo already supports conversation IDs, account switching and memory recovery. Verify the
complete PWA flows with both real providers: exhausted account → select another authenticated
account → continue the same conversation. Keep initial sign-in separate. Describe pause as
CPU relief and stopping resumable agents as memory relief. Current restart/update recovery
continues conversations after restarting agents; it does not promise uninterrupted processes.

Acceptance: sleep the phone, change networks, interrupt one peer and continue from another
device. Preserve host, conversation and draft identity. Also test an update and report
whether child processes survived or were restarted and resumed.

### 5. Let the phone review delivered work

wherdr documents touch-friendly diff selection and review comments assembled into a message
draft before sending. Source: [files and changes](https://github.com/afloury/wherdr/blob/main/docs/files.md).

Add a small read-only changed-files/diff view through bounded API and SDK methods. Restrict
reads to the selected project/worktree, handle symlinks and binary/large files, and render
paths/code as text. Bind review comments to a diff revision and keep them if the diff changes.
Compose feedback for the agent. Let agents use existing host tools for commits, PRs and
publishing; retain explicit user direction for external actions. This needs no mandatory
file browser or editor in the core.

## Implementation and verification boundaries

Complete one phone workflow at a time. Keep pure parsing in `core/`, I/O in services and
all new public capability in API plus SDK. Keep dependencies and per-session overhead small.
Task coordinators, boards and specialist orchestration can build on these primitives outside
the core. Do not add competitor features solely to match a checklist.

For each change, record the affected product acceptance scenario, API/SDK/PWA availability,
scope/owner behaviour and selected-host routing. Run required Rust checks and the headless
build for code changes. Distinguish fixture/browser checks from real phone, Linux, peer and
provider checks. None of the proposed improvements above is marked complete by this review.
