# Demo kit

A real kimo with an invented world, for screenshots, the website and trying kimo without
your own accounts, projects or machines.

```sh
cargo build                                        # the window is needed for window shots
node scripts/demo/run.mjs                          # open the printed address
node scripts/demo/run.mjs --capture                # every screenshot → docs/screenshots
node scripts/demo/run.mjs --capture phone          # only the phone ones (or: window)
node scripts/demo/run.mjs --capture --only phone-map,app-files
```

- [`world.mjs`](world.mjs) is the world: seven machines on the tailnet `demo.ts.net`
  (`100.101.x.x`), three phones, projects, terminal screens, accounts on `example.com`, plan
  limits, history and fleet jobs. Nothing in it is real. The website's 3D scene uses it too
  (`demoApi(world)` in [`web/js/map-demo.js`](../../web/js/map-demo.js)).
- [`run.mjs`](run.mjs) seeds a temporary `HOME` (git repositories, stand-in `claude`,
  `codex`, shell and `tailscale` programs, usage logs), starts `target/debug/kimo` on it
  (port + 1) and a proxy on `--port` (7790) that answers from the world what one machine
  cannot have: the tailnet, paired kimos, the fleet, node facts and plan limits.
- Shutdown is `true`, accounts hold no credentials and plan limits are never read from the
  real login.
- Phone shots use headless Chrome (430 × 932, scale 2, touch); window shots use the
  window's `KIMO_CAPTURE` hook. Both are taken in light and dark.

When you change a view, retake its screenshot (`--only`), look at it, and keep the world
consistent: the phone, the window and the website tell one story.
