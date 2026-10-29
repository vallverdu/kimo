# kimo logo and splash motion

The existing logo already contained Bézier outlines. On 2026-10-26 it was opened,
named and saved as `kimo.ai` with the **VectorCraft MCP**, then exported as the
brand master `kimo.svg`. Its original brush contours were preserved.

| File | Purpose |
| --- | --- |
| `kimo.ai` | Editable master, saved by VectorCraft in PDF-compatible AI format. |
| `kimo.svg` | Static vector master, exported from VectorCraft after saving the AI. |
| `kimo-animation.ai` | Editable, separate hair stick, bun, head and eye paths. This is artwork, not an animation timeline. |
| `kimo-parts.svg` | The same separated artwork in SVG; generated from the master. |
| `kimo-splash.svg` | Transparent, self-contained animation. 4.2 seconds, once, then holds. |
| `kimo-splash-preview.gif` | White-background review export at 20 fps; production uses the transparent SVG. |
| `../../web/icons/kimo-splash.svg` | Identical production copy, served by kimo as `/icons/kimo-splash.svg`. |
| `preview.html` | Offline preview with replay, pause, scrubbing, theme and still controls. Open directly in a browser. |
| `kimo-eyes.svg` | Finished bun and stick; only the eyes move up and down in a 4.8-second loop. |
| `../../web/icons/kimo-eyes.svg` | Identical production copy, served as `/icons/kimo-eyes.svg`. |
| `eyes-preview.html` | Offline eye-animation preview with the same playback and theme controls. |
| `kimo-eyes-preview.gif` | Looping white-background review export at 20 fps. |
| `kimo-connected.svg` | Phone and two kimos exchanging signals in an 8.4-second loop. |
| `../../web/icons/kimo-connected.svg` | Identical production copy, served as `/icons/kimo-connected.svg`. |
| `connected-preview.html` | Offline connected-device animation preview. |
| `kimo-connected-preview.gif` | Looping white-background review export at 20 fps. |
| `kimo-connected-artwork.svg` | Static composition with editable device frames, connections and logo instances. |
| `kimo-connected.ai` | Static composition saved through VectorCraft in PDF-compatible AI format. |
| `motion-preview.template.html` | Shared source template for the three generated HTML previews. |

The loose lengths sway before lifting, one slightly behind the other. They turn
around the root and tuck into the bun. The hair stick slides behind the bun,
slightly overshoots and settles, with a small answering tilt of the head. The
timing uses individually sculpted poses, curved trajectories and eased motion.
The finished silhouette retains the master's contours, including the brush tips.

The second animation, `kimo-eyes.svg`, starts with the finished logo. Both smiling
eyes move together: centre, up, centre, down, centre. The motion eases into short
pauses and repeats every 4.8 seconds. The original eye curves are translated
vertically without changing their shape; the head, bun and stick stay fixed.
Its framing matches the first animation's finished pose so an app can switch
from the hair animation to the eye loop without repositioning the logo.

The third animation, `kimo-connected.svg`, represents connected kimos across
devices. A phone client, a laptop kimo and a headless kimo form a small network.
Teal signals follow curved links from the phone to the laptop, on to the host,
then back to the phone. Arrival rings and small, eased nods give each logo a
response. The devices and their kimos remain in place. The scene repeats every
8.4 seconds, with a quiet interval at the seam. It is a brand illustration of
remote control and paired hosts; it does not depict moving projects between them.

The network SVG is transparent, `1200 × 900`, and uses the same light/dark and
reduced-motion conventions. Reduced motion shows the three connected devices
with no moving signals or nods. Its 18.3 KB source embeds one copy of the master
logo and reuses it within each device; it needs no external assets or libraries.
Use `/icons/kimo-connected.svg` for this variant. The static artwork resolves
colours to RGB values for editors that do not support CSS custom properties.

## Use in a splash screen

```html
<img class="splash-logo" src="icons/kimo-splash.svg"
     width="192" height="200" alt="kimo">
```

Use `icons/kimo-eyes.svg` as the source for the repeating eye animation. Its
reduced-motion fallback keeps the eyes in their original centre position.

The SVG uses native SVG animation (SMIL); it has no JavaScript, external fonts,
images or animation-library dependency. It plays once when loaded and keeps the
finished pose. Reload/recreate the image to replay it. Its transparent artboard
is `1374 × 1434`, with room for the lifting hair and the inserted stick.

It follows the embedding element's `color-scheme` (ink on light, white on dark).
Set `color-scheme: light` or `color-scheme: dark` on the image/container when the
app's explicit theme differs from the system theme. When inlining the SVG,
override its `color` to use the app's ink token. Keep its IDs unique if inlining
more than one copy. `prefers-reduced-motion: reduce` shows the finished static
logo immediately, with no moving framing. The base path/transform attributes
also provide a finished pose in renderers without SMIL.

The production asset is available in the PWA's static files. This change prepares
the artwork; it does not add a startup overlay or impose a minimum loading delay.
The operating system's launch screen for an installed PWA remains static; the
animation is for the in-app splash after the page loads.

## Editing and verification

After an edit to the static master, regenerate the separated artwork, the three
animated SVGs, their PWA copies and all previews' inline artwork with:

```sh
python3 scripts/make-logo-animation.py
```

The generator uses only Python's standard library and expects the master's
absolute M/L/C/Z contours and the existing hair/stick junctions. A redesign of
those junctions requires updating the separation points and authored poses.
Edit `motion-preview.template.html` to change the previews' shared layout or
controls. The AI files are saved separately through VectorCraft; the generator
does not rewrite them. The GIFs are browser-rendered review snapshots and are
also exported separately. Existing icon PNGs and pure black/white README variants
remain separate static derivatives.

Verified on macOS in headless Google Chrome, 2026-10-26:

- Both AI files reopened in VectorCraft with restored editable paths and no
  warnings; no raster image is embedded in either SVG artwork.
- Nine timeline samples were visually reviewed, from long hair to the held logo.
- The preview opened directly from a local file. Replay, pause, scrubbing and
  still controls worked. The 430 × 932 phone viewport fit in light and dark,
  with no horizontal overflow or JavaScript errors.
- The standalone production SVG selected its white static pose under dark theme
  and reduced motion. The assembled final pose was compared with the master at
  1054 × 1024: 16 antialiasing pixels differed, with alpha RMS below 0.18/255.
- An isolated headless kimo served `/icons/kimo-splash.svg` with the SVG content
  type and bytes identical to the brand source. Regeneration was deterministic.
- The eye loop was reviewed at centre, up, down and across the loop seam. Browser
  geometry checks confirmed vertical offsets of 0, -36 and +30 source units,
  no horizontal eye movement, and a fixed head/bun/stick. Playback continued
  beyond the first loop; pause, replay, scrubbing and still controls worked.
  Both previews passed the offline, light/dark phone-viewport and reduced-motion
  checks without page errors. `/icons/kimo-eyes.svg` was also verified through
  an isolated headless kimo.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` (112
  tests) and `cargo build --no-default-features` passed.

Adobe Illustrator, Safari and real iOS/Android devices were not exercised. The
preview uses browser phone emulation. No API, SDK or host-routing behavior changed.

Connected-device animation verified on macOS in headless Chrome, 2026-10-27:

- The interactive preview opened directly from a local file. Signals followed all
  three paths; replay, pause, scrubbing, still mode and repeated playback worked.
  Screenshots at 0 and 8.4 seconds were identical. Six timeline samples and the
  dark phone layout were visually reviewed.
- Widths 320, 390, 430 and 768 CSS pixels fit without horizontal overflow in light
  and dark. The standalone SVG selected its white, still connected scene under
  dark theme and reduced motion. No JavaScript page errors were reported.
- The original splash and eye previews still loaded and paused correctly after
  the shared template gained configurable artwork width and download links.
- VectorCraft opened the static scene, saved `kimo-connected.ai` and reopened it
  with restored editable vectors and no conversion warnings. Illustrator itself
  was not exercised.
- An isolated headless kimo served `/icons/kimo-connected.svg` with the correct
  SVG content type and bytes identical to the brand source. All three animations
  and previews regenerated deterministically. Clippy, all 130 Rust tests and the
  headless build passed on the current working tree.
- Repository-wide `cargo fmt --check` reported existing formatting differences in
  the unrelated file-browser Rust work (`src/api/files.rs` and related files).
  Those edits were left untouched. This artwork does not modify Rust sources.

These checks cover animation and presentation. Actual paired-host workflows,
Safari and physical mobile devices were not exercised for this artwork task.
