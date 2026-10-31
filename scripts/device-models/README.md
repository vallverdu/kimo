# Device models

The map and the native window draw machines from these files when a kimo holds them, and from
their own shapes when it does not. They live in each owner's `~/.kimo/models`, not in this
repository: most vendors' CAD files do not allow redistribution.

```bash
python3 -m venv .venv
.venv/bin/pip install -r scripts/device-models/requirements.txt
.venv/bin/python scripts/device-models/build.py ~/.kimo/models/source/models.json --out ~/.kimo/models
```

Keep the CAD sources in `~/.kimo/models/source/` (one folder per device, plus `models.json`):
kimo serves only the built `*.glb` files next to `index.json`, never that folder, and it is
outside every repository.

Needs `node`/`npx` too, for simplifying meshes. `models.json` (see `models.example.json`) names a
STEP or GLB file for each `kind`, relative to itself:

| Key | Meaning |
|---|---|
| `source` | the CAD file: `.step`, `.stp` or `.glb` (meshopt-compressed is fine) |
| `label` | its name |
| `tris` | triangle budget (default 15000) |
| `rotate` | `[x, y, z]` degrees, if it does not arrive standing up |
| `base` | `[r, g, b]`, 0–1, to paint a model that has no colours |

Kinds the clients know: `nucExtreme`, `nucEnthusiast`, `nuc13Extreme`, `rpi4`, `rpi5`, `zero2`,
`jetson`. What it does to each: tessellates the CAD, flattens and joins it, simplifies, drops the
smallest parts if that is not enough (thousands of tiny closed parts cannot be simplified, and
no one sees them at map scale), then stands it on y = 0, centred. The result is plain glTF in
metres, +Y up, uncompressed, so the web client and the Rust window read it with no decoder
(`web/js/glb.js`, `src/core/glb.rs`).

kimo serves them at `GET /api/models` and `GET /api/models/{file}`. Check the licence of what you
download before sharing a pack.
