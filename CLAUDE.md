# kimo: conventions for agents

Shared architecture: `~/templates/foundation/ARCHITECTURE.md`. The web layer follows
`~/templates/web-timber` (copied into `web/`; `css/tokens.css` is generated from the shared tokens:
never hard-code colours or font sizes, use the `--color-*` variables).

## Layers (dependencies point down)

```
src/main.rs        entry point only: config → Manager → axum server
src/api/           HTTP/websocket handlers and static files; no business rules
src/services/      I/O: sessions.rs (PTYs, lifecycle), accounts.rs, store.rs (only file touching state.json), stats.rs, network.rs
src/core/          pure, synchronous, no I/O: models, lifecycle, launch, procfs, scrollback, network parsing
src/config.rs      clap flags with env fallbacks (see .env.example)
web/sdk/           public SDK for any PWA: kimo.js (API + events), kimo-terminal.js (<kimo-terminal>)
web/js/api.js      kimo's own page: an SDK instance with the owner token; app.js renders; terminal.js wraps <kimo-terminal>
src/core/access.rs who may call what from which origin (keep it pure and tested)
```

## Rules

- Pure logic goes in `core/` with unit tests next to it; process behaviour is covered in `tests/sessions.rs`.
- Keep it light: no new heavy dependencies, no per-session async tasks beyond the two PTY threads.
- State is plain JSON in `~/.kimo/state.json`, unknown fields preserved (`#[serde(flatten)] extra`).
- Credentials never go in state or git: they live in each account's config home.
- Every API change goes through the SDK too: kimo's own PWA uses only `web/sdk/`, so other apps get the same features.
- Errors reach the user as `{ "error": "plain words" }` and a toast; details go to the log.
- Run `cargo fmt`, `cargo clippy --all-targets` and `cargo test` before committing.
- TODO.md is the task list: tick what you finish, add follow-ups you discover.
