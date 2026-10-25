# Third-party notices

kimo is licensed under MIT OR Apache-2.0 (see `LICENSE-MIT`, `LICENSE-APACHE`). It bundles:

| Component | Where | License |
|---|---|---|
| [xterm.js](https://github.com/xtermjs/xterm.js) 5.5.0 and addon-fit 0.10.0 | `web/vendor/xterm/` | MIT (`web/vendor/xterm/LICENSE`) |
| [Roboto Mono](https://fonts.google.com/specimen/Roboto+Mono) | `assets/fonts/RobotoMono.ttf` | SIL Open Font License 1.1 (`assets/fonts/OFL.txt`) |

Rust dependencies are listed in `Cargo.lock`; each keeps its own license (all permissive:
MIT, Apache-2.0, BSD, ISC, Zlib, Unicode or similar). `cargo license` (from the `cargo-license`
crate) prints them.

The web client loads Roboto Mono from Google Fonts when it has internet access, and falls
back to the system monospace font.
