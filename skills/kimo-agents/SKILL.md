---
name: kimo-agents
description: Work with the other coding agents running in kimo (on this machine and on paired machines): list them, send them a message, read their answer, wait for them, or start a helper agent. Use it when asked to coordinate, delegate to, ask, review with or check on another agent, and when a message arrives that starts with "[kimo message from agent".
---

# Working with other agents through kimo

You run inside kimo, which runs other agents (Claude Code, Codex, shells) on this machine and
on paired machines. The `kimo` command talks to them. `KIMO_SESSION` is your own id.

## Commands

```bash
kimo whoami                       # your id, name and folder
kimo agents                       # every agent: ID, NAME, KIND, STATE, RESUME ID, FOLDER
kimo send <id|name> "message"     # type a message into another agent (signed with your id)
kimo wait <id|name>               # block until it stops working (idle, needs input or stopped)
kimo read <id|name>               # its last answer (from its own transcript)
kimo read <id|name> --screen      # its recent terminal screen instead
kimo new claude --name reviewer --prompt "Review src/parser.rs and report problems"
kimo new codex --cwd ~/dev/api --prompt "…"   # start a helper agent in another folder
```

Ids from paired machines look like `gpu-lab/api-refactor-1a2b`; use them the same way.

## How to collaborate

1. `kimo agents` to see who exists and what they are doing. STATE is `working`, `idle`,
   `needs input` (waiting for an answer or an approval) or `stopped`.
2. Send one clear, self-contained request: the goal, the files, what to report back.
3. `kimo wait <id>` then `kimo read <id>` for the answer. Don't poll in a tight loop.
4. When you receive a `[kimo message from agent NAME (id ID) …]`, do the task, then answer with
   `kimo send ID "…"` so the other agent gets your result.

## Rules

- Don't send messages to an agent that `needs input`: the person decides there.
- Don't approve prompts or press keys for other agents; only send messages.
- Keep messages short; point to files instead of pasting large content.
- Start helper agents only when the task asks for it, and tell the person which ones you started.
