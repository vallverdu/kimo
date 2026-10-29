# Requirements audit — 2026-10-29

> **Update:** findings F1–F8 were fixed in `ebaccab` (0.1.15), each with a regression test or a
> browser check; see the requirements-audit section of [TODO.md](../TODO.md). The product
> acceptance scenarios below that need real devices and providers remain open.

**Result: kimo does not yet satisfy all its defined requirements.** The automated checks
pass, but targeted probes reproduced failures in fleet execution, shutdown safety, selected
host continuity and disconnected phone input. Several product acceptance scenarios also
remain incomplete or unverified on real devices/providers.

## Scope and evidence

Requirements: [product direction](product-direction.md), [AGENTS.md](../AGENTS.md), the
priority section of [TODO.md](../TODO.md), [fleet guide](fleet.md), README and the fleet
architecture evaluation's Phases 1–4. Proposed later features are not treated as regressions.

Baseline: `bdc0d05`, package version 0.1.14, including the unreleased fleet implementation.
The starting working tree also contained concurrent map, documentation and artwork changes.
The main agent began further discovery/setup work during this audit. Rust failure probes
were therefore pinned to a temporary source archive of `bdc0d05`; PWA probes used the
working tree's actual JavaScript. Revalidate findings after concurrent changes land.

Host: macOS, Apple silicon. All simulated hosts used temporary data/config homes and
loopback addresses. Jobs were harmless shell commands. Shutdown was replaced by `touch`
of a temporary marker. No actual machine was switched off, real account changed, or real
agent commanded. Browser checks used Chrome with a 390 × 844 touch viewport and fixture
API data, not a real phone. SSE was stubbed in the host-race probe to isolate HTTP responses.

Evidence retained locally under `/tmp/kimo-audit-20261029/`:

- `checks.json`, `fmt.log`, `clippy.log`, `test.log`, `headless.log`;
- `harness/src/main.rs`, `reproduction-results.json`, `reproduction.log`;
- `pwa-audit.mjs`, `pwa-results.json`, `pwa-host-race.png`.

The temporary harness is audit instrumentation, not a committed regression suite. It uses
a source archive to avoid interfering with the other agent's implementation.

## Reproduced failures

### F1 — P1: an uncertain dispatch can execute a job twice

Requirement: durable jobs, bounded retries/failover and exclusive resource ownership.

Location: `fleet/src/services/fleet.rs`, `dispatch()`; `Fleet::open()` also resets
`Dispatched` jobs to `Ready` after a coordinator restart. Worker `create_job()` rejects an
already existing id rather than reconciling it.

Probe: two real isolated kimo API servers; a proxy forwards the first job creation to A,
then returns 503 after A has accepted it. With `retries: 0`, the coordinator excludes A and
dispatches to B. **Both A and B report the same worker id running**, while the coordinator
reports only B and `attempt: 1`.

A failed response does not establish that the worker did not start. Persist the intended
worker/attempt before sending, reconcile the deterministic id on that worker, and retain
an unknown-outcome state instead of immediately placing another copy elsewhere. Apply the
same rule to coordinator restart and staged upload/start failures. Test acknowledgement loss
after acceptance and restart during dispatch; a side-effect marker must be written once.

### F2 — P1: a non-forced shutdown can run after new work starts

Requirement: interactive work wins; the machine refuses shutdown while sessions/jobs/logins
keep it on.

Location: `src/services/power.rs`, `Manager::power_off()`.

Probe: request shutdown while idle, then immediately start a 20-second shell session during
the 1.5-second shutdown delay. **The shutdown marker runs while the session is `Running`.**
The blockers were checked only before sleeping.

Coordinate draining with session/job creation and perform a final blocker check before the
command. Report an aborted shutdown and clear `shutting_down` when new work intervenes.
Cover a new job as well as a terminal; use a stand-in command in every test.

### F3 — P1: a workflow can succeed without its declared input

Requirement: dependency steps pass stored artifacts to the consumer before it starts.

Locations: `fleet/src/core/jobs.rs`, `check_workflow()`;
`fleet/src/services/fleet.rs`, `dispatch()` input loop.

Probe: producer declares and creates `data.txt`; consumer declares
`inputs: {model: producer/missing.txt}`. Submission is accepted. Dispatch logs that the input
is missing and continues. **The consumer prints `MISSING` and the workflow is `succeeded`.**

Check that an input names a declared producer output. At dispatch, fail or block explicitly
when a required artifact is unavailable; do not run the consumer without it. Make optional
missing inputs explicit if supported. Test missing, unavailable and failed-producer inputs.

### F4 — P1: cancelling an unreachable worker releases its lease without stopping it

Requirement: accurate cancellation and exclusive resource leases until work has ended.

Location: `fleet/src/services/fleet.rs`, `cancel()` and candidate resource accounting.

Probe: start a detached worker job, disconnect its API server while leaving the job alive,
then cancel from the coordinator. **Coordinator: `cancelled`; worker: `running`; device
lease list: empty.** The remote cancellation error is discarded. This permits capacity to
be reused even though the original work can still run after connectivity returns.

Keep cancellation pending and capacity reserved until the worker acknowledges a terminal
state or an explicit reconciliation/fencing policy resolves ownership. Retry cancellation
after reconnect and report uncertainty to the PWA. Cover cancellation during dispatch too.

### F5 — P1: a late response replaces another selected host's projects

Requirement: keep host identity clear and preserve the destination of projects/actions.

Location: `web/js/app.js`, `refresh()` and `connect()`; also inspect asynchronous project,
account, memory and settings callbacks for the same issue.

Probe: hold A's projects response, select B, allow B to render, then release A's response.
**The selector still says B, but the visible project list becomes A's.** The browser reports
no JavaScript errors. SSE does not repair the project list because its callback updates only
sessions/accounts. Actions subsequently consult the current mutable API host.

Use a host/connection generation and an immutable destination client for each operation.
Ignore stale responses and subscriptions. Keep creation results and drafts bound to their
original host; do not open an old-host session using the newly selected host's client.
Test rapid switching, delayed responses, stale authentication failures and reconnects.

### F6 — P2: submitting during a terminal disconnect silently loses the draft

Requirement: reconnect to ongoing work and reliably direct it from a phone.

Locations: `web/js/terminal.js`, compose submit;
`web/sdk/kimo-terminal.js`, `sendBytes()`.

Probe: open a fixture terminal, close its actual WebSocket, and submit before reconnect.
**Socket state is CLOSED; the non-empty composer becomes empty.** `sendBytes()` silently
returns when the socket is not open, while the composer also clears attachments. No send
failure reaches the user.

Preserve the draft/attachments and show disconnected or uncertain delivery. Disable sending
until connected or use a bounded queue with explicit delivery status. Avoid automatically
replaying a possibly delivered command. Test close before text, between text and Enter,
and while an attachment is uploading.

### F7 — P2: a registry capacity overrides smaller reported hardware

Requirement: actual reported facts must satisfy job requirements before placement.

Locations: `fleet/src/core/schedule.rs`, `qualifies()`;
`fleet/src/services/fleet.rs`, `candidates()`.

Pure scheduling probe: registry says 12 GB VRAM, online facts report 6 GB, job needs 10 GB.
**Choice is `Run("overstated-registry")`.** `qualifies()` uses the larger value. Candidate
construction also uses total RAM/VRAM rather than currently available capacity.

Use registry capacity as an offline planning hint, with live facts authoritative for an
online machine. Define whether memory requirements concern installed or available capacity
and apply that consistently. Account for active reservations. Test contradictory/stale
registry values and multiple jobs rather than assuming the GPU is free because kimo has
no lease for it.

### F8 — P2: the documented HTTPS notification route cannot work with this build

Requirement: fleet notifications, including the guide's `ntfy: https://ntfy.sh/...` example.

Locations: `fleet/Cargo.toml`, `Cargo.toml`, `fleet/src/services/net.rs::notify()`.

The Rust HTTP dependencies enable JSON but no TLS backend. A request to an isolated live
loopback server using an HTTPS URL returns **`TLS required, but transport is unsecured`**.
The notification sender uses the same TLS-disabled ureq build. Thus the documented HTTPS
ntfy example cannot deliver; no external notification service was contacted in this probe.
HTTPS/WSS peer support is an existing acknowledged gap too.

Provide a certificate-validating transport for these paths or an explicit supported
alternative. Surface failed delivery and test HTTPS success/failure with a local TLS fixture.
Keep credentials on encrypted transports; do not silently downgrade HTTPS to HTTP.

## Further requirement gaps found by source review

- **Approval evidence is not shown.** `JobSpec.show` is deserialized but not consumed by the
  coordinator or phone view. `web/js/fleet.js::jobRow()` links only a job's own artifacts;
  approval steps have none. A documented approval such as `show: [results/jetson.json]`
  therefore offers approve/reject without presenting that evidence. Resolve the intended
  producer artifacts, expose them through API/SDK and render them before approval. This is
  source evidence; no real-phone approval session was performed.
- **Guided creation is unfinished.** `web/js/projects.js` still creates the folder/project
  separately; its project action directly creates a session without an account/initial-task
  step. Existing backend operations work, but the top-priority guided journey remains open.
- **Limits and mobile setup remain incomplete.** Usage bars and manual account switching
  exist. The TODO items for exhausted-limit detection/recovery, phone-friendly Codex sign-in
  and phone settings/skills remain open; they cannot be counted as accepted workflows.
- **Documentation is not a current acceptance record.** The product-direction source-review
  table still describes the 2026-10-26 implementation, and README screenshots remain disclosed
  v0.1.8 fixtures. Keep these dated records distinct from verification of the current fleet,
  file/review and process-handoff features. Do not infer completion from checked inventory
  items or demo screenshots.

## Product acceptance matrix

“Partial” means supporting code/tests exist; it does not certify an end-to-end phone journey.
No real-device scenario is marked failed solely because this audit could not exercise it.

| Defined scenario | Result and evidence |
| --- | --- |
| Start from a phone | Partial: new/existing/clone project API tests pass; guided agent/account/task creation remains separate. Real-phone run not verified. |
| Deliver remotely | Partial: terminal control, messaging, read-only phone files/diffs and fleet jobs exist. F1/F3/F6 and missing approval evidence prevent a blanket pass. Real-provider publishing not exercised. |
| Reconnect | Partial with reproduced F6: terminal retry and SSE snapshots exist; disconnected sending loses drafts. Real phone sleep/network changes not verified. |
| Continue on another device | Partial: conversation IDs and peer routing tests pass. F5 breaks trustworthy selected-host presentation. NUC → Mac/tablet continuation not exercised. |
| Control several hosts | Partial: four peer integration tests pass, including revocation. F5 remains; encrypted peer transport and real two-host PWA checks remain open. |
| Change accounts | Partial: stand-in Claude switching and Codex resume tests pass. Both real providers and mobile switching were not exercised. |
| Recover from a limit | Incomplete: explicit TODO for detecting exhaustion and offering a suitable switch remains open. Usage display alone does not satisfy it. |
| Recover memory | Partial: idle-agent stop/resume and resource reporting tests pass. Mobile recovery on real hardware and overhead under several real agents were not measured. |
| Build on kimo | Partial: scope/origin tests and the Node SDK script pass; previous host binding fixes for SDK send/upload are present. An independent browser's complete scoped project → agent → observe workflow remains unverified. |
| Stay light and headless | Headless build passes. Small dependencies/layering are preserved in the reviewed areas; current release overhead under multiple sessions and representative Linux/GPU hosts was not benchmarked. |

Fleet Phases 1–4 have working happy-path tests for one-way pairing, guarded power, command
jobs, workflow artifacts and approvals. F1–F4/F7/F8 and missing approval evidence prevent
calling those phases fully accepted. Phase 0 hardware/energy/setup verification remains
open. Phase 5 hardening is explicitly later work, not claimed complete by this audit.

## Checks and limits

At the starting working-tree snapshot:

- `cargo fmt --all --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo test --workspace`: **178 passed, 1 ignored, 0 failed**. The ignored test is the
  live Codex plan-limit test, `services::usage::live::codex_answers_its_plan_limits`.
- `cargo build --no-default-features`: passed.

Earlier owner-access, state-permission, peer/device revocation, CORS and SDK send/upload
host-binding fixes are present; their included regression tests passed. This audit does not
relabel those older findings as unfixed.

Real iOS/Android, real account/provider runs, Linux execution, NUC Wake-on-LAN/poweroff,
Jetson/Hailo/GPU facts, Pi deployment and ZeroTier operation were not verified. Follow the
existing hardware checklist rather than presenting loopback stand-ins as that evidence.
The main agent's concurrent discovery/setup work began after the green workspace checks;
those checks are not a claim that later in-progress edits were tested.

## Handoff

Prepared for the existing main Claude implementation agent, as requested by the owner.
Sent through Orca to the existing `Kimo Rust agent development environment` terminal.
Request `15cd0beb-dc09-498e-a8fc-90e717046298` returned `accepted: true` / `input_accepted`.
Claude was already working on discovery/setup; a new turn start was not observed during
the 10-second submission check. Receipt: `/tmp/kimo-audit-20261029/claude-handoff-receipt.json`.
This confirms input acceptance, not completed fixes or an acknowledgement from Claude.

Prioritise F1–F5, then disconnected input and the other contract gaps. Add regressions for
these scenarios and update the relevant API, SDK, PWA and TODO evidence as fixes land.
This review makes no implementation changes.
