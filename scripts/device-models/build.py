#!/usr/bin/env python3
"""Build kimo's device models: CAD (STEP) or a GLB -> one small GLB per device, plus index.json.

The map and the native window draw these instead of their procedural shapes. The result is
plain glTF (metres, +Y up, centred on x/z, resting on y = 0, no compression), so kimo's web
client and its Rust window both read it without a decoder.

They are not part of kimo's repository: most vendor CAD files do not allow redistribution, so
each owner builds their own pack from files they downloaded, into ~/.kimo/models (or --out).

    python3 -m venv .venv && .venv/bin/pip install -r scripts/device-models/requirements.txt
    .venv/bin/python scripts/device-models/build.py models.json --out ~/.kimo/models

models.json: {"<kind>": {"source": "file.step|file.glb", "label": "...", "tris": 15000,
"rotate": [x_deg, y_deg, z_deg], "base": [r, g, b], "shell_only": ["group", ...]}}. Needs
node/npx for mesh simplification (@gltf-transform/cli).
"""
import argparse
import json
import math
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
import trimesh

GLTF_TRANSFORM = ["npx", "--yes", "@gltf-transform/cli@4.1.1"]


def step_to_glb(step: Path, out: Path, tolerance_mm: float) -> None:
    import cascadio

    code = cascadio.step_to_glb(
        str(step), str(out), tol_linear=tolerance_mm / 1000.0, tol_angular=0.6, tol_relative=False
    )
    if code != 0 or not out.exists():
        raise RuntimeError(f"could not read {step} (cascadio returned {code})")


def gltf(*args: str) -> None:
    run = subprocess.run([*GLTF_TRANSFORM, *args], capture_output=True, text=True)
    if run.returncode != 0:
        raise RuntimeError(f"gltf-transform {args[0]}: {run.stderr.strip() or run.stdout.strip()}")


def triangles(path: Path) -> int:
    scene = trimesh.load(str(path), force="scene")
    return sum(len(g.faces) for g in scene.dump())


def parts_of(faces: np.ndarray, vertex_count: int) -> np.ndarray:
    """Which connected part each face belongs to (a resistor, a pin, a shell)."""
    parent = np.arange(vertex_count)

    def find(v):
        while parent[v] != v:
            parent[v] = parent[parent[v]]
            v = parent[v]
        return v

    for a, b, c in faces.tolist():
        ra, rb, rc = find(a), find(b), find(c)
        parent[rb] = ra
        parent[find(rc)] = ra
    return np.array([find(f[0]) for f in faces.tolist()])


def drop_small_parts(src: Path, out: Path, target_tris: int) -> None:
    """Thousands of tiny closed parts are what a simplifier cannot reduce and what no one sees
    at map scale: drop the smallest until the model fits its budget, biggest parts kept."""
    scene = trimesh.load(str(src), force="scene")
    meshes = {name: scene.geometry[name] for name in scene.geometry}
    world = np.linalg.norm(scene.bounds[1] - scene.bounds[0])
    parts = []  # (size, name, faces)
    for name, g in meshes.items():
        ids = parts_of(np.asarray(g.faces), len(g.vertices))
        corners = np.asarray(g.vertices)[np.asarray(g.faces)]
        order = np.argsort(ids, kind="stable")
        ids_sorted = ids[order]
        starts = np.flatnonzero(np.r_[True, ids_sorted[1:] != ids_sorted[:-1]])
        for group in np.split(order, starts[1:]):
            pts = corners[group].reshape(-1, 3)
            parts.append((float(np.linalg.norm(pts.max(0) - pts.min(0))), name, group))
    parts.sort(key=lambda p: -p[0])
    # Never drop what is visible at a glance, whatever the budget says.
    keep_always = world * 0.08
    kept, total = {name: [] for name in meshes}, 0
    for size, name, group in parts:
        if size < keep_always and total + len(group) > target_tris * 2:
            continue
        kept[name].append(group)
        total += len(group)
    for name, groups in kept.items():
        g = meshes[name]
        if groups:
            g.update_faces(np.isin(np.arange(len(g.faces)), np.concatenate(groups)))
            g.remove_unreferenced_vertices()
        else:
            scene.delete_geometry(name)
    scene.export(str(out))


def rotation(deg) -> np.ndarray:
    m = np.eye(4)
    for axis, angle in zip(np.eye(3), deg):
        if angle:
            m = trimesh.transformations.rotation_matrix(math.radians(angle), axis) @ m
    return m


def normalise(src: Path, out: Path, rotate, base) -> dict:
    """Bake the rotation, stand it on y = 0, centre it on x/z, and paint unpainted parts."""
    scene = trimesh.load(str(src), force="scene")
    scene.apply_transform(rotation(rotate))
    low, high = scene.bounds
    offset = np.eye(4)
    offset[:3, 3] = [-(low[0] + high[0]) / 2, -low[1], -(low[2] + high[2]) / 2]
    scene.apply_transform(offset)
    if base:
        paint = trimesh.visual.material.PBRMaterial(
            baseColorFactor=[*base, 1.0], metallicFactor=0.0, roughnessFactor=0.9
        )
        for g in scene.geometry.values():
            g.visual = trimesh.visual.TextureVisuals(material=paint)
    scene.export(str(out))
    low, high = scene.bounds
    return {"size_m": [round(float(v), 4) for v in high - low]}


def build(kind: str, spec: dict, root: Path, out_dir: Path, tolerance_mm: float) -> dict:
    source = (root / spec["source"]).expanduser()
    if not source.exists():
        raise FileNotFoundError(source)
    target_tris = int(spec.get("tris", 15000))
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        raw = tmp / "raw.glb"
        if source.suffix.lower() in (".step", ".stp"):
            step_to_glb(source, raw, tolerance_mm)
        else:
            # Decoded here, so a meshopt-compressed source is read like any other.
            gltf("copy", str(source), str(raw))
        flat = tmp / "flat.glb"
        gltf("flatten", str(raw), str(flat))
        joined = tmp / "joined.glb"
        gltf("join", str(flat), str(joined))
        welded = tmp / "welded.glb"
        gltf("weld", str(joined), str(welded))
        before = triangles(welded)
        simple = tmp / "simple.glb"

        def reduce(src: Path) -> None:
            # The error budget grows only as far as it must, so detail survives when it can.
            ratio = min(1.0, target_tris / max(triangles(src), 1))
            for error in (0.001, 0.003, 0.01, 0.03, 0.06, 0.1):
                gltf("simplify", str(src), str(simple), "--ratio", f"{ratio:.5f}", "--error", str(error))
                if triangles(simple) <= target_tris * 1.15:
                    return

        reduce(welded)
        if triangles(simple) > target_tris * 1.15:
            culled = tmp / "culled.glb"
            drop_small_parts(simple, culled, target_tris)
            again = tmp / "again.glb"
            gltf("weld", str(culled), str(again))
            reduce(again)
        clean = tmp / "clean.glb"
        gltf("prune", str(simple), str(clean))
        final = out_dir / f"{kind}.glb"
        info = normalise(clean, final, spec.get("rotate", [0, 0, 0]), spec.get("base"))
        info |= {
            "kind": kind,
            "file": final.name,
            "label": spec.get("label", kind),
            "triangles": triangles(final),
            "source_triangles": before,
            "bytes": final.stat().st_size,
        }
    return info


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("spec", type=Path, help="models.json")
    ap.add_argument("--out", type=Path, default=Path("~/.kimo/models"))
    ap.add_argument("--only", nargs="*", help="only these kinds")
    ap.add_argument("--tolerance-mm", type=float, default=0.15, help="CAD tessellation tolerance")
    args = ap.parse_args()
    specs = json.loads(args.spec.read_text())
    out_dir = args.out.expanduser()
    out_dir.mkdir(parents=True, exist_ok=True)
    index_path = out_dir / "index.json"
    models = {}
    if index_path.exists():
        models = {m["kind"]: m for m in json.loads(index_path.read_text()).get("models", [])}
    failed = 0
    for kind, spec in specs.items():
        if args.only and kind not in args.only:
            continue
        print(f"{kind}: {spec['source']}", flush=True)
        try:
            models[kind] = build(kind, spec, args.spec.parent, out_dir, args.tolerance_mm)
            m = models[kind]
            print(f"  {m['source_triangles']} -> {m['triangles']} triangles, {m['bytes'] >> 10} KB, {m['size_m']} m", flush=True)
        except Exception as e:  # keep going: one bad file should not lose the others
            failed += 1
            print(f"  FAILED: {e}", file=sys.stderr, flush=True)
    tmp = index_path.with_suffix(".tmp")
    tmp.write_text(json.dumps({"models": sorted(models.values(), key=lambda m: m["kind"])}, indent=2))
    os.replace(tmp, index_path)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
