# Third-party notices

kimo is licensed under MIT OR Apache-2.0 (see `LICENSE-MIT`, `LICENSE-APACHE`). It bundles:

| Component | Where | License |
|---|---|---|
| [xterm.js](https://github.com/xtermjs/xterm.js) 5.5.0 and addon-fit 0.10.0 | `web/vendor/xterm/` | MIT (`web/vendor/xterm/LICENSE`) |
| [Roboto Mono](https://fonts.google.com/specimen/Roboto+Mono) | `assets/fonts/RobotoMono.ttf` | SIL Open Font License 1.1 (`assets/fonts/OFL.txt`) |
| [DejaVu Sans Mono](https://dejavu-fonts.github.io/) (symbol fallback) | `assets/fonts/DejaVuSansMono.ttf` | Bitstream Vera / Arev license, DejaVu changes public domain (`assets/fonts/DejaVu-LICENSE.txt`) |
| [Noto Sans Symbols 2](https://github.com/notofonts/symbols) (symbol fallback) | `assets/fonts/NotoSansSymbols2-Regular.ttf` | SIL Open Font License 1.1 (`assets/fonts/NotoSansSymbols2-OFL.txt`) |

Rust dependencies are listed in `Cargo.lock`; each keeps its own license (all permissive:
MIT, Apache-2.0, BSD, ISC, Zlib, Unicode or similar). `cargo license` (from the `cargo-license`
crate) prints them. `kimo-fleet` also builds in TLS (for notifications and ZeroTier Central):
[rustls](https://github.com/rustls/rustls) (Apache-2.0, ISC or MIT),
[ring](https://github.com/briansmith/ring) (Apache-2.0 and ISC, with code from BoringSSL under
its notices) and [webpki-roots](https://github.com/rustls/webpki-roots) (Mozilla's root
certificates, CDLA-Permissive-2.0).

The web client loads Roboto Mono from Google Fonts when it has internet access, and falls
back to the system monospace font.
