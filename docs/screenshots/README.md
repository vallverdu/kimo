# README screenshots

These screenshots show **v0.1.15** (the Map and Fleet tabs, the window's files and changes),
captured on 2026-10-29 with the demo kit in [`scripts/demo/`](../../scripts/demo/README.md).
Each view has a light PNG and a matching `-dark.png`. The top logo uses transparent SVGs with
the master's original geometry: `kimo-light.svg` is pure black (`#000000`) and
`kimo-dark.svg` is pure white (`#ffffff`). The brand master stays in `assets/brand/kimo.svg`.

The README uses paired links ending in `#gh-light-mode-only` and `#gh-dark-mode-only` so
GitHub's selected theme controls the images. Other Markdown viewers may show both variants.

## Everything in them is invented

The machines (studio, gpu-box, builder, vision-pi, jetson, watch-pi, travel-air), the phones,
the tailnet `demo.ts.net` and its `100.101.x.x` addresses, the `192.168.50.x` LAN, the
projects, terminals, accounts (`example.com`), plan limits, history and fleet jobs all come
from [`scripts/demo/world.mjs`](../../scripts/demo/world.mjs). No real account, key, address,
device or project appears.

## How they are made

```sh
cargo build                                   # the window captures use target/debug/kimo
node scripts/demo/run.mjs --capture           # phone and window, light and dark
node scripts/demo/run.mjs --capture phone     # only the phone views
```

- A real kimo runs with an isolated `HOME` and data folder, seeded with the invented
  projects (git repositories with changes), stand-in `claude`, `codex` and shell programs
  that draw a fixed screen, two accounts and Claude/Codex usage logs. Terminals, files,
  changes, history and usage are kimo's own answers.
- A small proxy in front of it answers what cannot be real on one machine from the same
  world: the tailnet, the paired kimos and their projects, node facts, the fleet
  coordinator and plan limits. It shows the temporary `HOME` as `/Users/demo`.
- Phone views: headless Chrome through the DevTools protocol, a 430 × 932 CSS-pixel mobile
  viewport, touch, device scale factor 2. This is emulation, not a real iOS or Android device.
- Window views: the egui window itself; `KIMO_CAPTURE` (with `KIMO_CAPTURE_VIEW`,
  `_SESSION`, `_PANEL`, `_HISTORY`, `_WAIT`) opens a view, saves the frame and closes.

Screenshots demonstrate the UI; they do not establish real provider behaviour, cross-host
routing, performance or product acceptance. Retake them for a release with visible changes,
keeping names and aspect ratios in each light/dark pair, and check every README reference.
