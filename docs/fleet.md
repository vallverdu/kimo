# kimo fleet: several machines, one coordinator

kimo runs on every machine (the node agent: terminals, agents, jobs). **kimo-fleet** runs on
one always-on machine (a Raspberry Pi 4 here) and coordinates them:

- it keeps the registry of machines and their power state;
- it wakes machines with Wake-on-LAN and switches them off when idle;
- it places jobs and workflows on the machine that fits;
- it keeps their logs and output files, and asks you for approvals.

The phone pairs once, with the kimo on the Pi, and reaches all of this in the **Fleet** tab.

```
phone ──► kimo (Pi 4) ──/api/fleet──► kimo-fleet (Pi 4, 127.0.0.1:7780)
                                           │  fleet keys, HTTP over ZeroTier/LAN
                                           ├─► kimo on the NUCs     (woken, switched off)
                                           ├─► kimo on the Pi 5, Jetson (always on)
                                           └─► kimo on the MacBook  (used when awake)
```

The design and its reasons are in the evaluation document this was built from.

## What the coordinator can do on a machine

A machine trusts the coordinator **one way**. Pairing with `role: fleet` gives the coordinator
a key for these calls only:

- `GET /api/status` and `/api/node` (facts: GPUs, Jetson, Hailo, Xcode, load, disk, logins);
- `/api/jobs` (run commands and headless agents);
- `/api/power` (switch off, never forced).

It cannot reach terminals, sessions, files, accounts, keys or other kimos, and it is never
usable from a web page. The machine gets no key for the coordinator.

Revoke the key in the machine's Devices list; it stops working at once.

> A fleet key runs jobs, and a job is a command run as you on that machine. Pair only your
> own coordinator.

## Setting it up

1. **Every machine** runs `kimo serve` as a service (`contrib/kimo.service` on Linux,
   `contrib/ai.kimo.plist` on macOS), bound to the network the Pi reaches it on:
   `KIMO_BIND=zerotier` (or a LAN address).
2. **The Pi** runs `kimo-fleet` (installed with kimo; `contrib/kimo-fleet.service`).
   - Its kimo needs `KIMO_FLEET=http://127.0.0.1:7780`.
   - The coordinator listens on 127.0.0.1 only. Requests from the machine itself are its
     owner (with the same DNS-rebinding guard as kimo); `KIMO_FLEET_TOKEN` lets scripts in
     from elsewhere.
3. **Describe the machines** in `~/.kimo-fleet/devices.yaml` (created with an example on
   first start), then run `kimo-fleet reload`.
4. **Pair each machine.** On the machine run `kimo pair`, or use Devices → show QR code. The
   code is under the QR. On the Pi run `kimo-fleet pair <id> <code>`, or tap *pair* in the
   Fleet tab.
5. **Wake-on-LAN** (NUCs):
   - In the BIOS, set Wake-on-LAN from S4/S5 to Power On.
   - Turn off "Wake on USB from S5" if the machine restarts after a Linux shutdown.
   - Set "After power failure" to Stay off.
   - The magic packet leaves the Pi on its LAN (`broadcast:`), because ZeroTier does not
     carry it.
6. **Switching off** (Linux): kimo runs `systemctl poweroff`. A user service has no seat,
   so polkit may refuse it. kimo then tries `sudo -n systemctl poweroff`, which works with a
   sudoers rule:

   ```
   # sudo visudo -f /etc/sudoers.d/kimo-poweroff
   jordi ALL=(root) NOPASSWD: /usr/bin/systemctl poweroff
   ```

   Other setups can set `KIMO_SHUTDOWN_COMMAND`. Test it once from the Fleet tab:
   *switch off*.

## Adding machines from the network

The coordinator lists the machines on your Tailscale network (`tailscale status`) and on
ZeroTier. For ZeroTier it uses Central's member list when the data folder has a
`zerotier-token` file or `KIMO_FLEET_ZEROTIER_TOKEN` is set; otherwise it looks at the hosts
of its ZeroTier network (a /24 at most) where kimo or SSH answers.

Each machine shows:

- whether its kimo answers and whether SSH does;
- the hardware model it looks like, and the role that model suggests.

Tap **find machines** in the Fleet tab, or run `kimo-fleet discover`.

**Adding** a machine (*add* there, or `kimo-fleet add <address>`) goes like this:

1. **Over SSH**, with your keys or Tailscale SSH, never a password, the coordinator runs a
   short script on the machine:
   - it installs kimo (the coordinator's own release) if it is missing;
   - it runs kimo as a service bound to the network the machine was found on (systemd user
     service on Linux, LaunchAgent on macOS);
   - it gets a pairing code from that kimo. A machine is its own kimo's owner, so this needs
     no typing.

   A machine whose kimo already runs can join with a pairing code instead of SSH.
2. It **pairs one way** and reads the machine's facts.
3. It gives the machine its **hardware model**: the one you chose, or the one the facts
   point to. It also gives it a **role**, and for machines it can wake, the wired card on
   the coordinator's own network.
4. It writes the entry to `devices.d/<id>.yaml`. Your `devices.yaml` is never rewritten.
   *remove* deletes that file, and the key on both sides.

To let the coordinator in over SSH, do one of these once:

- `ssh-copy-id <user>@<machine>` from the coordinator (*find machines* shows its key);
- turn on Tailscale SSH on the machine: `tailscale set --ssh`.

On Linux, the script asks for `loginctl enable-linger` so kimo keeps running after you log
out; it tells you when that needs `sudo`.

A kimo older than 0.1.15 knows no fleet keys and would hand out an owner key. The coordinator
notices, revokes that key and stops, so update kimo there first.

### Hardware models and roles

| Model | Role | Power | Notes |
|---|---|---|---|
| `nuc-11-extreme` | gpu-worker | woken, off when idle | BIOS: Wake-on-LAN from S5 |
| `nuc-11-enthusiast` | build-worker | woken, off when idle | same BIOS settings |
| `linux-gpu-pc` | gpu-worker | woken, off when idle | `ethtool -s <card> wol g` |
| `jetson-orin-nano` | edge-target | always on | no Wake-on-LAN |
| `rpi5`, `rpi5-hailo` | agent-host | always on | about 3 W |
| `rpi4` | control-plane | always on | takes no jobs |
| `macbook-air`, `mac-mini`, `mac` | apple-builder | never woken | used when awake |
| `linux-pc` | build-worker | left on | set Wake-on-LAN once it works |

- **Roles:** control-plane, agent-host, gpu-worker, build-worker, edge-target,
  apple-builder, spare. A role is also a label, so a job can ask for `gpu-worker`.
- **The entry.** The model gives the labels, power policy, slots and idle watts. The
  machine's own facts fill in GPU memory, total memory and the Wake-on-LAN card.
- **Without a card.** A machine with no wired card on the coordinator's network is never
  switched off, since it could not be woken again.
- **The map** draws each machine from its model (`kind`). `kimo-fleet models` lists them.

## devices.yaml

```yaml
ntfy: https://ntfy.sh/my-fleet-topic       # notifications (optional)
devices:
  nuc-extreme:
    kimo: http://10.147.17.21:7777          # its kimo
    mac: "1c:69:7a:aa:bb:cc"                # Wake-on-LAN
    broadcast: 192.168.1.255                # the LAN's broadcast address (default 255.255.255.255)
    power: { wake: wol, sleep: shutdown, idle_minutes: 20, boot_deadline_s: 180, boots_per_day: 10 }
    labels: [x86_64, linux, cuda]
    resources: { gpu: 1, vram_gb: 12, cpu_slots: 4, memory_gb: 32 }
    cost: 40                                # idle watts: cheaper machines are chosen first
    model: nuc-11-extreme                   # hardware model (optional; the map draws it)
    role: gpu-worker                        # also a label (optional)
  macbook-air:
    kimo: http://10.147.17.30:7777
    power: { wake: never, sleep: never }    # used when awake, never woken
    labels: [macos, arm64, xcode]
```

- **`labels`** add to what the machine reports itself:
  - `x86_64`/`arm64`, `linux`/`macos`
  - `cuda`, `jetson` + `tensorrt`, `rpi`, `hailo-10h`, `xcode`
- **`resources`** are held exclusively by the jobs that ask for them. A machine runs
  `cpu_slots` jobs at once (default 2).
- A mistake in the file is reported, in the Fleet tab and by `kimo-fleet reload`, and the
  machines stay as they were.

## Power

A machine's state is one of:

| State | Meaning |
|---|---|
| `off` | not answering, and not expected to |
| `waking` | a magic packet went out |
| `booted` | pings, but its kimo is not ready |
| `ready` | up |
| `busy` | jobs on it |
| `idle` | up, no jobs: switched off after `idle_minutes` |
| `draining` | switching off |
| `unreachable` | should be up and isn't |
| `manual` | you took control |

- **Waking.** The coordinator waits for the machine's kimo and its facts. With
  `resources.gpu`, the GPU must be visible. A wake gets `boot_deadline_s` and one more magic
  packet; then the machine is `unreachable` and you are notified.
- **Switching off.** A machine is switched off when all of these hold for `idle_minutes`:
  - it holds no jobs;
  - nothing runs there (agents, terminals, jobs of its own, someone logged in);
  - no queued job could go to it within 30 minutes.

  Its kimo checks again and refuses if anything runs. If it still answers two minutes
  later, you are notified.
- **Overrides** (Fleet tab or `kimo-fleet hold`):
  - keep it on for some hours;
  - never switch it off;
  - manual: nothing automatic;
  - wake now, or switch off now (refused while jobs run there).
- **Limits.** At most `boots_per_day` automatic cold boots. Machines that cannot be woken
  (laptops, boards) are used when they answer, and jobs for them wait.

## Jobs and workflows

A job is a command or a headless agent. In YAML, or as JSON through the API:

```yaml
task: benchmark-model
runner: command                  # command | agent | approval
run: ./bench.sh --model {{inputs.model}}
requires: { labels: [arm64, jetson], memory_gb: 4, vram_gb: 10 }
resources: { gpu: 1, cpu_slots: 1 }   # held exclusively while it runs
git: { repo: ~/dev/seg, ref: main, branch: "kimo/{{job}}" }   # a fresh checkout (or cwd:)
env: { BATCH: "8" }
timeout: 30m
retries: 1
prefer: nuc-extreme              # or device: (only that one)
not_before: 1761699600000        # Unix ms; its machine is woken 3 minutes ahead
power: { wake_if_needed: true }
```

- **Agent jobs** (`runner: agent`, `agent: claude | codex`, `prompt:`, `model:`, `effort:`,
  `permissions: yolo`) run `claude -p` or `codex exec` and end with the agent's answer.
  Unattended agents usually need `permissions: yolo`. Keep that off on machines that hold
  signing keys.
- **Where a job runs.** Each job runs in a fresh folder of its own, in `cwd:`, or in a
  checkout of a commit (`git:`): a worktree of a repository on the machine, or a clone of
  a URL.
- **Retries.** A failed job is retried on the same machine (`retries:`). A job lost with its
  machine is retried on another.

**When the network is not reliable:**

- A run's id (`<job>-<attempt>`) is written down before the job is sent. If the machine's
  answer does not come, the coordinator asks the machine about that id before doing
  anything else:
  - if the machine has it, the run is adopted;
  - if the machine says it does not, the job is placed again under the same id;
  - if the machine cannot be asked, the job waits as *dispatched* and keeps its lease.
- A job that loses its machine moves on. The run it may have left there is ended when the
  machine answers again, and its GPU stays reserved until then.
- **Cancelling** a job on a machine shows *cancelling* until the machine confirms the run
  ended.

A **workflow** is a list of steps:

```yaml
workflow: model-to-release
steps:
  - task: train
    requires: { labels: [cuda], vram_gb: 10 }
    run: python train.py --export onnx
    outputs: [out/model.onnx]
  - task: bench-jetson
    needs: [train]
    requires: { labels: [jetson] }
    inputs: { model: train/out/model.onnx }   # put at out/model.onnx before it starts
    run: ./bench_trt.sh {{inputs.model}}
    outputs: [results/jetson.json]
  - task: approve-release
    runner: approval
    needs: [bench-jetson]
    show: [results/jetson.json]
```

- **Needs.** A step starts when the steps it `needs` have succeeded. If one of them failed,
  it is skipped, unless the failed step is `optional`.
- **Outputs.** When a step succeeds, the coordinator copies its `outputs` from the machine
  and keeps them under their SHA-256. Later steps get them as `inputs`, at the same path,
  even when the machine that made them is off by then.
- **Approvals** wait for you in the Fleet tab, or `kimo-fleet approve <job>`. The files
  their `show:` names, from earlier steps, come with them, and small text files are shown in
  place.
- An input must be one of the `outputs` of the step it names. A step whose input was not
  made (its step failed, even an optional one) fails rather than running without it.

Run them with `kimo-fleet run file.yaml`, from the Fleet tab (+), or through the SDK:
`kimo.fleet.submit(job)` and `kimo.fleet.submitWorkflow(yaml)`.

## Commands

```text
kimo-fleet serve                 the coordinator (default)
kimo-fleet devices               machines, states, labels, GPUs, problems
kimo-fleet discover              machines on Tailscale and ZeroTier (kimo? SSH? model)
kimo-fleet add <address> [--model m --role r --user u --id id --code CODE --reinstall]
kimo-fleet remove <id>           a machine added with `add` (entry and keys)
kimo-fleet models                hardware models and roles
kimo-fleet pair <id> <code>      pair with a machine (code from `kimo pair` there)
kimo-fleet wake <id>             Wake-on-LAN
kimo-fleet shutdown <id>         switch off (refused while work runs there)
kimo-fleet hold <id> --hours 2 | --never | --manual | --auto
kimo-fleet run <file.yaml>       a job or a workflow
kimo-fleet jobs | log <job> | cancel <id> | approve <job> [--reject]
kimo-fleet events [--device id]  wakes, shutdowns, jobs, approvals
kimo-fleet reload                read devices.yaml again
kimo-fleet backup <file>         a copy of fleet.db (safe while it runs)
```

## The coordinator's API

All calls are under `/api` on the coordinator, and under `/api/fleet` on its kimo.

| Method | Path | |
|---|---|---|
| GET | `/status` | `{version, host, devices, registry, registry_error, jobs: {ready, waiting_device, running, awaiting_approval}}` |
| GET, PUT | `/registry` | `devices.yaml` as text; a PUT is checked first |
| POST | `/reload` | read `devices.yaml` again |
| GET | `/devices`, `/devices/{id}` | machines: `{id, state, power, labels, facts, jobs, paired, can_wake, auto_shutdown, shutdown_at, keep_on_until, never_auto_shutdown, manual, boots_today, last_seen, error, registry}` |
| POST | `/devices/{id}/wake`, `/shutdown` | wake it; switch it off |
| POST | `/devices/{id}/hold` | `{hours?, never?, manual?}` |
| POST | `/devices/{id}/pair` | `{code}`; DELETE `/devices/{id}/key` forgets the key |
| GET | `/hardware` | hardware models and roles |
| GET | `/discover` | machines on Tailscale and ZeroTier: `{tailscale, zerotier, notes, machines: [{name, address, network, os, online, self, can_run_kimo, id, in_fleet, kimo, ssh, model, role}]}` |
| POST | `/adopt` | `{address, id?, model?, role?, ssh_user?, network?, code?, reinstall?}` → 202; follow with GET `/adoptions/{id}` (`{state, steps, notes, error}`) |
| DELETE | `/devices/{id}` | remove a machine added from the Fleet tab |
| GET | `/ssh-key` | the coordinator's public SSH key |
| GET | `/devices/{id}/samples?hours=` | its facts every 30 s (kept 14 days) |
| GET | `/events?device=&job=&limit=` | wakes, shutdowns, state changes, jobs, approvals (kept 14 days) |
| GET, POST | `/jobs` | jobs; queue one (JSON or YAML) |
| GET | `/jobs/{id}`, `/jobs/{id}/log?offset=` | a job; its log (from its machine while it runs, kept here after) |
| POST | `/jobs/{id}/cancel`, `/approve`, `/reject` | |
| GET | `/jobs/{id}/artifacts/{path}` | an output it made |
| GET, POST | `/workflows`, `/workflows/{id}`, POST `/workflows/{id}/cancel` | runs and their jobs |

## Where things are

- **The coordinator's data dir** (`~/.kimo-fleet/`):
  - `devices.yaml`
  - `keys.json` (mode 600)
  - `fleet.db` (SQLite: events, samples, machine states, jobs, workflows)
  - `logs/`
  - `artifacts/` (unused ones go after 30 days)
- **Each machine:** `~/.kimo/jobs/<id>/` holds the job's spec, its log, its exit and its
  work folder. kimo keeps the newest 200 jobs.

## What was verified, and what was not

**Verified by automated tests** (`fleet/tests/fleet.rs`, `tests/api.rs`), with real kimos as
machines on local ports:

- one-way pairing and what a fleet key may do;
- the magic packet (caught on a local socket);
- a lost answer after the machine took a job (it runs once), a coordinator restart with
  jobs on their way, cancelling while the machine does not answer, and missing inputs;
- a job waking a machine, running there, and its output kept;
- idle shutdown through the machine's own check, and refusal while work runs there;
- holds and manual control;
- a workflow passing files between machines, with a retry, an optional failure, a headless
  agent (a stand-in `claude`) and an approval;
- jobs in git worktrees and clones;
- jobs surviving the kimo that started them.

**Verified by hand** on a Mac: the Fleet tab in a phone-sized browser (pair state, approve, a
job's log, a job queued from the phone).

**Not verified:**

- real Wake-on-LAN on the NUCs;
- `systemctl poweroff` under a user service;
- `nvidia-smi`, Jetson and Hailo facts on real hardware;
- Claude and Codex in headless jobs;
- ARM builds on the Pi;
- ZeroTier between real machines.
