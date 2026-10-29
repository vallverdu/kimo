#!/usr/bin/env python3
"""Build transparent splash, eye-motion and connected-kimo SVGs from the master.

No animation library or raster frames. The finished pose uses the master's exact
Bézier contours. Run after editing assets/brand/kimo.svg: python3 scripts/make-logo-animation.py
"""

import bisect
import json
import math
import re
from pathlib import Path
from xml.etree import ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
BRAND = ROOT / "assets/brand"
NS = "{http://www.w3.org/2000/svg}"
DURATION = 4.2


def curves(data):
    """Absolute M/L/C/Z paths, as emitted by VectorCraft, to cubic subpaths."""
    tokens = re.findall(r"[MLCZ]|[-+]?(?:\d*\.\d+|\d+)(?:[eE][-+]?\d+)?", data)
    paths, segments, start, point = [], [], None, None
    i = 0
    while i < len(tokens):
        command = tokens[i]
        i += 1
        if command == "Z":
            if point != start:
                segments.append((point, point, start, start))
            paths.append(segments)
            segments, point = [], start
            continue
        count = {"M": 2, "L": 2, "C": 6}[command]
        values = list(map(float, tokens[i:i + count]))
        i += count
        end = tuple(values[-2:])
        if command == "M":
            start = end
        elif command == "L":
            segments.append((point, point, end, end))
        else:
            segments.append((point, tuple(values[:2]), tuple(values[2:4]), end))
        point = end
    return paths


def bridge(a, b, c=None, d=None):
    return a, c or a, d or b, b


def path(segments):
    def point(p):
        return " ".join(f"{v:.3f}".rstrip("0").rstrip(".") if v else "0" for v in p)
    return "M" + point(segments[0][0]) + " ".join(
        "C" + " ".join(point(p) for p in s[1:]) for s in segments
    ) + "Z"


def sample(segment, t):
    return tuple(sum((1-t)**3 * segment[0][j] if k == 0 else
                     3*(1-t)**2*t * segment[1][j] if k == 1 else
                     3*(1-t)*t*t * segment[2][j] if k == 2 else
                     t**3 * segment[3][j] for k in range(4)) for j in range(2))


def measure(segments):
    points = [segments[0][0]]
    distances, boundaries = [0.0], [0.0]
    for s in segments:
        for i in range(1, 61):
            p = sample(s, i / 60)
            distances.append(distances[-1] + math.dist(points[-1], p))
            points.append(p)
        boundaries.append(distances[-1])
    total = distances[-1]
    return points, [d / total for d in distances], [d / total for d in boundaries], total


def retopologize(pose, reference):
    """Match posed curves to the final outline's anchors for native SVG morphing.

    Sampling by arc length keeps the master's tiny brush nicks from introducing
    sudden jumps. The last pose is the original geometry, without resampling.
    """
    points, distances, _, length = measure(pose)
    _, _, fractions, _ = measure(reference)

    def at(t):
        t = max(0, min(1, t))
        i = min(bisect.bisect_right(distances, t), len(distances) - 1)
        span = distances[i] - distances[i - 1]
        f = (t - distances[i - 1]) / span if span else 0
        return tuple(a + (b-a)*f for a, b in zip(points[i-1], points[i]))

    def tangent(t):
        a, b = at(t - .0001), at(t + .0001)
        norm = math.dist(a, b)
        return tuple((y-x) / norm if norm else 0 for x, y in zip(a, b))

    result = []
    for lo, hi in zip(fractions, fractions[1:]):
        a, b = at(lo), at(hi)
        u, v = tangent(lo), tangent(hi)
        arm = length * (hi-lo) / 3
        result.append((a, tuple(x+y*arm for x, y in zip(a, u)),
                       tuple(x-y*arm for x, y in zip(b, v)), b))
    return result


def morph(poses, times, final, easing=".42 0 .3 1"):
    values = [path(retopologize(curves(p)[0], final)) for p in poses]
    values.extend([path(final), path(final)])
    times = [*times, 1]
    return (f'<animate attributeName="d" dur="{DURATION}s" fill="freeze" '
            f'calcMode="spline" keyTimes="{";".join(map(str, times))}" '
            f'keySplines="{";".join([easing] * (len(times)-1))}" '
            f'values="{";".join(values)}"/>')


def longer(pose):
    """Length below the shoulders; the crown and the face stay undisturbed."""
    return path([tuple((x, y + max(0, y - 850) * .65) for x, y in s)
                 for s in curves(pose)[0]])


def preview(svg, filename, title, subtitle, asset, duration, loop, stages, art_width=390,
            ai_asset="kimo.ai", svg_asset="kimo.svg"):
    """Offline previews share controls, layout and accessible playback."""
    html = (BRAND / "motion-preview.template.html").read_text()
    values = {
        "ARTWORK": svg,
        "TITLE": f"kimo · {title}",
        "HEADING": f"kimo / {title}",
        "SUBTITLE": subtitle,
        "ASSET": asset,
        "AI_ASSET": ai_asset,
        "SVG_ASSET": svg_asset,
        "ART_WIDTH": str(art_width),
        "DURATION": str(duration),
        "CAPTION": f"{duration} seconds · " + ("seamless loop" if loop else "plays once, then holds"),
        "TIMELINE": json.dumps({"duration": duration, "loop": loop, "stages": stages}),
    }
    for key, value in values.items():
        html = html.replace("{{" + key + "}}", value)
    (BRAND / filename).write_text(html)


def build_eyes(outer, opening, left_eye, right_eye):
    """Only translate the original eye outlines; all other contours stay fixed."""
    eyes = f'<path id="left-eye" d="{path(left_eye)}"/><path id="right-eye" d="{path(right_eye)}"/>'
    svg = f'''<svg id="kimo-eyes-logo" xmlns="http://www.w3.org/2000/svg" width="1374" height="1434" viewBox="-160 -180 1374 1434" role="img" aria-labelledby="eyes-title eyes-desc">
  <title id="eyes-title">kimo · eye motion</title>
  <desc id="eyes-desc">The finished kimo logo. Its smiling eyes gently move up, pause, move down, and return to centre. The bun, hair stick and head remain still.</desc>
  <!-- Generated by scripts/make-logo-animation.py. Transparent SVG, 4.8-second loop. -->
  <style>
    #kimo-eyes-logo {{ color: #232323; }}
    @media (prefers-color-scheme: dark) {{ #kimo-eyes-logo {{ color: #ffffff; }} }}
    #kimo-eyes-logo .still {{ display: none; }}
    @media (prefers-reduced-motion: reduce) {{ #kimo-eyes-logo .motion {{ display: none; }} #kimo-eyes-logo .still {{ display: inline; }} }}
  </style>
  <g fill="currentColor" fill-rule="evenodd">
    <path id="finished-logo" d="{path(outer)} {path(opening)}"/>
    <g id="moving-eyes" class="motion">
      <animateTransform attributeName="transform" type="translate" dur="4.8s" repeatCount="indefinite" calcMode="spline"
        keyTimes="0;.10;.28;.38;.53;.63;.82;.92;1"
        values="0 0;0 0;0 -36;0 -36;0 0;0 0;0 30;0 30;0 0"
        keySplines="0 0 1 1;.42 0 .35 1;0 0 1 1;.42 0 .45 1;0 0 1 1;.42 0 .35 1;0 0 1 1;.42 0 .45 1"/>
      {eyes}
    </g>
    <g class="still">{eyes.replace('id="left-eye"', 'id="still-left-eye"').replace('id="right-eye"', 'id="still-right-eye"')}</g>
  </g>
</svg>
'''
    (BRAND / "kimo-eyes.svg").write_text(svg)
    (ROOT / "web/icons/kimo-eyes.svg").write_text(svg)
    preview(svg, "eyes-preview.html", "eye motion", "bun in place · eyes up and down",
            "kimo-eyes.svg", 4.8, True,
            [[0, "Centre"], [.48, "Up"], [1.83, "Centre"], [3.02, "Down"], [4.42, "Centre"]])
    print(f"Wrote {len(svg.encode()):,}-byte eye-motion SVG (4.8 s, loop).")


def build_network(silhouette):
    """A phone controls two paired hosts; every kimo stays on its own device."""
    duration = 8.4
    routes = [
        ("phone-to-computer", "M308 500 C400 500 390 305 550 305", .07, .25),
        ("computer-to-host", "M820 445 C820 505 886 488 886 550", .30, .46),
        ("host-to-phone", "M780 610 C650 790 232 784 232 638", .54, .78),
    ]

    def animate(attribute, values, times, spline=".42 0 .35 1"):
        return (f'<animate attributeName="{attribute}" dur="{duration}s" repeatCount="indefinite" '
                f'calcMode="spline" values="{";".join(map(str, values))}" '
                f'keyTimes="{";".join(map(str, times))}" '
                f'keySplines="{";".join([spline] * (len(times)-1))}"/>')

    def logo(cx, cy, width, arrival, moving):
        height = width * 1024 / 1054
        reaction = ""
        if moving:
            times = [0, arrival, arrival+.035, arrival+.075, arrival+.125, 1]
            values = [f"{a} {width/2:.3f} {height/2:.3f}" for a in [0, 0, -3, 1, 0, 0]]
            reaction = (f'<animateTransform attributeName="transform" type="rotate" dur="{duration}s" '
                        f'repeatCount="indefinite" calcMode="spline" '
                        f'keyTimes="{";".join(map(str, times))}" values="{";".join(values)}" '
                        f'keySplines="{";".join([".42 0 .35 1"] * (len(times)-1))}"/>')
        return (f'<g transform="translate({cx-width/2:.3f} {cy-height/2:.3f})"><g>{reaction}'
                f'<use href="#network-kimo" width="{width}" height="{height:.3f}"/></g></g>')

    def port(x, y, arrival, moving):
        pulse = ""
        if moving:
            times = [0, arrival, arrival+.005, arrival+.07, arrival+.13, 1]
            pulse = (f'<circle cx="{x}" cy="{y}" r="7" fill="none" '
                     'stroke="var(--network-signal)" stroke-width="2.4" opacity="0">'
                     + animate("r", [7, 7, 7, 21, 21, 21], times)
                     + animate("opacity", [0, 0, .7, .25, 0, 0], times) + '</circle>')
        return (f'<circle cx="{x}" cy="{y}" r="5" fill="var(--network-signal)"/>' + pulse)

    def scene(moving):
        links, packets = [], []
        for name, route, start, end in routes:
            links.append(f'<path d="{route}" fill="none" stroke="currentColor" stroke-width="3.5" opacity=".22"/>')
            if moving:
                times = [0, start, start+.012, end-.012, end, 1]
                links.append(f'<path d="{route}" fill="none" stroke="var(--network-signal)" stroke-width="3.5" opacity="0">'
                             + animate("opacity", [0, 0, .45, .45, 0, 0], times) + '</path>')
                packets.append(f'''<g data-route="{name}" opacity="0">
          <animateMotion dur="{duration}s" repeatCount="indefinite" calcMode="spline"
            path="{route}" keyTimes="0;{start};{end};1" keyPoints="0;0;1;1"
            keySplines="0 0 1 1;.42 0 .35 1;0 0 1 1"/>
          {animate("opacity", [0, 0, 1, 1, 0, 0], times)}
          <circle r="13" fill="var(--network-signal)" opacity=".12"/>
          <circle r="6.5" fill="var(--network-signal)"/>
        </g>''')
        return f'''
      <g stroke-linecap="round" stroke-linejoin="round">{"".join(links)}</g>
      <g aria-label="Phone client" data-device="phone">
        <g fill="none" stroke="currentColor" stroke-width="3.5" stroke-linecap="round" opacity=".7">
          <rect x="156" y="368" width="152" height="270" rx="24"/>
          <path d="M213 385 H251 M211 619 H253"/>
        </g>
        {logo(232, 499, 103, .78, moving)}
      </g>
      <g aria-label="Computer kimo" data-device="computer">
        <g fill="none" stroke="currentColor" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round" opacity=".7">
          <rect x="550" y="170" width="420" height="275" rx="17"/>
          <path d="M550 445 L526 464 C524 469 533 473 546 473 H974 C987 473 996 469 994 464 L970 445 M714 445 L720 453 H800 L806 445"/>
          <circle cx="760" cy="184" r="2" fill="currentColor" stroke="none"/>
        </g>
        {logo(760, 309, 159, .25, moving)}
      </g>
      <g aria-label="Headless kimo" data-device="host">
        <g fill="none" stroke="currentColor" stroke-width="3.5" stroke-linecap="round" opacity=".7">
          <rect x="780" y="550" width="212" height="150" rx="24"/>
          <path d="M799 678 H815 M825 678 H841"/>
          <circle cx="971" cy="678" r="2" fill="currentColor" stroke="none"/>
        </g>
        {logo(886, 620, 93, .46, moving)}
      </g>
      {port(308, 500, .07, moving)}
      {port(550, 305, .25, moving)}
      {port(820, 445, .30, moving)}
      {port(886, 550, .46, moving)}
      {port(780, 610, .54, moving)}
      {port(232, 638, .78, moving)}
      {"".join(packets)}
'''

    css = '''
    #kimo-network { color: var(--color-ink, #232323); --network-signal: var(--color-teal, #12939a); }
    @media (prefers-color-scheme: dark) { #kimo-network { color: var(--color-ink, #ffffff); --network-signal: var(--color-teal, #3fb8bf); } }
    #kimo-network .still { display: none; }
    @media (prefers-reduced-motion: reduce) { #kimo-network .motion { display: none; } #kimo-network .still { display: inline; } }
'''
    defs = f'<defs><symbol id="network-kimo" viewBox="0 0 1054 1024"><path fill="currentColor" fill-rule="evenodd" d="{silhouette}"/></symbol></defs>'
    svg = f'''<svg id="kimo-network" xmlns="http://www.w3.org/2000/svg" width="1200" height="900" viewBox="0 0 1200 900" role="img" aria-labelledby="network-title network-desc">
  <title id="network-title">kimos, connected</title>
  <desc id="network-desc">A phone, a computer kimo and a headless kimo exchange signals along curved connections. Each kimo remains on its device and gently responds when a signal arrives.</desc>
  <!-- Generated by scripts/make-logo-animation.py. Transparent SVG, 8.4-second loop. -->
  <style>{css}</style>
  {defs}
  <g class="motion">{scene(True)}</g>
  <g class="still">{scene(False)}</g>
</svg>
'''
    (BRAND / "kimo-connected.svg").write_text(svg)
    (ROOT / "web/icons/kimo-connected.svg").write_text(svg)
    # A static composition is useful for artwork editors and non-animated placements.
    static = f'''<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="900" viewBox="0 0 1200 900" style="color:#232323;--network-signal:#12939a">
  <title>kimos, connected · artwork</title>
  {defs}{scene(False)}
</svg>
'''
    # Resolve colours for editors that do not implement CSS custom properties.
    static = static.replace("var(--network-signal)", "#12939a").replace("currentColor", "#232323")
    (BRAND / "kimo-connected-artwork.svg").write_text(static)
    preview(svg, "connected-preview.html", "kimos, connected",
            "a phone · two kimos · working together", "kimo-connected.svg", duration, True,
            [[0, "Ready"], [.59, "Phone"], [2.52, "Computer"], [4.54, "Host"], [6.56, "Together"]],
            art_width=760, ai_asset="kimo-connected.ai", svg_asset="kimo-connected-artwork.svg")
    print(f"Wrote {len(svg.encode()):,}-byte connected-kimo SVG (8.4 s, loop).")


def build():
    master = ET.parse(BRAND / "kimo.svg").getroot()
    silhouette = master.find(f".//{NS}path").attrib["d"]
    outer, opening, left_eye, right_eye = curves(silhouette)

    def endpoint(x, y):
        return next(i + 1 for i, s in enumerate(outer) if math.dist(s[3], (x, y)) < .05)

    left_base = endpoint(367, 315.9)
    left_lower = endpoint(344.6, 289.2)
    left_upper = endpoint(328.3, 224.7)
    right_upper = endpoint(766.4, 208.7)
    right_lower = endpoint(744.1, 264.2)
    right_base = endpoint(696, 314.1)
    a, b = outer[left_base-1][3], outer[right_base-1][3]
    # Overlap the hidden join so separate antialiased paths have no hairline seam.
    head = outer[:left_base] + [bridge(a, b, (445, 292), (612, 288))] + outer[right_base:]
    bun = (outer[left_base:left_lower]
           + [bridge(outer[left_lower-1][3], outer[left_upper-1][3], (331, 272), (327, 248))]
           + outer[left_upper:right_upper]
           + [bridge(outer[right_upper-1][3], outer[right_lower-1][3], (762, 229), (753, 250))]
           + outer[right_lower:right_base] + [bridge(b, a, (612, 294), (445, 298))])
    stick = (outer[left_lower:left_upper]
             + [bridge(outer[left_upper-1][3], outer[right_upper-1][3])]
             + outer[right_upper:right_lower]
             + [bridge(outer[right_lower-1][3], outer[left_lower-1][3])])

    # Sculpted poses: loose lengths, anticipation, lift, a turn around the root,
    # then the tucked knot. Each lock follows a different timing and arc.
    loose = """M430 340 C290 288 132 373 60 573 C-10 760 -15 1002 40 1145
      C56 1186 95 1213 123 1190 C106 1185 97 1158 103 1140
      C134 1190 183 1178 193 1137 C147 996 140 782 215 584
      C278 441 408 384 553 368 C531 349 475 338 430 340 Z"""
    anticipate = """M430 340 C284 294 117 382 44 576 C-30 775 -13 1004 57 1138
      C80 1184 115 1208 145 1181 C122 1176 115 1150 120 1132
      C156 1181 202 1161 211 1120 C161 990 140 780 215 584
      C278 441 408 384 553 368 C531 349 475 338 430 340 Z"""
    lift = """M432 326 C302 282 152 273 59 365 C-68 489 -62 688 54 781
      C111 826 179 832 218 796 C179 806 147 784 142 761
      C183 789 225 777 245 744 C129 674 118 505 232 418
      C321 350 433 370 552 360 C531 339 477 323 432 326 Z"""
    turn = """M421 323 C329 265 285 152 352 59 C422 -43 626 -55 758 41
      C817 85 845 142 822 191 C823 155 797 133 776 131
      C799 163 791 197 761 211 C723 85 557 57 457 136
      C376 203 440 282 562 329 C516 350 461 345 421 323 Z"""
    wrap = """M373 316 C332 284 314 216 335 163 C358 75 489 8 596 15
      C674 14 756 77 779 156 C785 179 782 203 770 214
      C775 193 767 177 754 169 C765 203 750 241 732 265
      C698 317 642 331 563 326 C503 321 432 307 373 316 Z"""
    main_morph = morph([longer(loose), longer(anticipate), lift, turn, wrap],
                       [0, .08, .29, .46, .57, .66], bun)

    # The second length trails the first, loops once, and ends entirely inside
    # the exact bun. It never dissolves or shrinks into a point.
    right_poses = [
        """M620 347 C785 279 952 377 1012 561 C1080 761 1060 1010 1006 1154
          C990 1200 951 1217 923 1184 C943 1181 952 1154 945 1134
          C918 1181 870 1171 859 1136 C910 981 915 771 845 584
          C794 449 690 391 561 368 C574 355 600 347 620 347 Z""",
        """M620 347 C795 278 971 374 1035 560 C1109 774 1095 1013 1034 1151
          C1009 1200 975 1212 944 1177 C965 1175 973 1150 966 1129
          C936 1173 887 1161 879 1125 C935 980 938 767 852 582
          C794 449 690 391 561 368 C574 355 600 347 620 347 Z""",
        """M616 334 C781 299 925 328 1005 433 C1100 568 1080 755 964 834
          C913 870 861 861 832 829 C858 838 885 821 893 802
          C865 824 831 815 816 792 C928 720 927 549 827 451
          C751 380 657 372 555 359 C575 343 598 335 616 334 Z""",
        """M612 324 C766 337 907 283 928 164 C955 23 807 -85 666 -62
          C604 -51 565 -16 557 22 C570 -2 600 -8 621 -4
          C590 3 577 32 589 52 C688 -4 817 74 804 164
          C790 254 673 278 555 326 C574 338 593 337 612 324 Z""",
        """M608 317 C712 291 768 209 741 125 C715 43 610 13 528 42
          C493 54 468 74 460 99 C472 82 491 77 505 79
          C485 85 476 102 480 117 C554 72 647 108 667 168
          C689 232 621 284 552 316 C568 325 591 324 608 317 Z""",
    ]
    right_poses[:2] = [longer(p) for p in right_poses[:2]]
    tucked = curves("M590 306 C680 287 738 218 715 150 C693 81 606 50 536 75 C505 87 489 104 486 124 C499 106 515 100 527 102 C509 110 505 126 511 140 C568 102 638 134 650 179 C666 232 610 273 550 304 C559 313 579 313 590 306 Z")[0]
    second_morph = morph(right_poses, [0, .12, .33, .49, .61, .7], tucked)

    # Authored static layers are also provided for editing in VectorCraft/AI.
    parts = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1054 1024" width="1054" height="1024">
  <title>kimo · separated animation artwork</title>
  <g fill="#232323" fill-rule="evenodd">
    <path id="hair-stick" d="{path(stick)}"/>
    <path id="bun" d="{path(bun)}"/>
    <path id="head-and-eyes" d="{path(head)} {path(opening)} {path(left_eye)} {path(right_eye)}"/>
  </g>
</svg>
'''
    (BRAND / "kimo-parts.svg").write_text(parts)

    svg = f'''<svg id="kimo-logo" xmlns="http://www.w3.org/2000/svg" width="1374" height="1434" viewBox="-160 -180 1374 1434" role="img" aria-labelledby="title desc">
  <title id="title">kimo</title>
  <desc id="desc">Loose hair gathers, twists into a bun and is secured with a hair stick. The logo settles and holds.</desc>
  <!-- Generated by scripts/make-logo-animation.py. Native SVG, transparent, 4.2 seconds, no loop. -->
  <style>
    #kimo-logo {{ color: #232323; }}
    @media (prefers-color-scheme: dark) {{ #kimo-logo {{ color: #ffffff; }} }}
    #kimo-logo .still {{ display: none; }}
    @media (prefers-reduced-motion: reduce) {{ #kimo-logo .motion {{ display: none; }} #kimo-logo .still {{ display: inline; }} }}
  </style>
  <defs>
    <mask id="face-clear" maskUnits="userSpaceOnUse" x="-160" y="-400" width="1374" height="2000" style="mask-type:luminance">
      <rect x="-160" y="-400" width="1374" height="2000" fill="white"/>
      <path d="{path(opening)}" fill="black"/>
    </mask>
  </defs>
  <g fill="currentColor" fill-rule="evenodd">
    <g class="motion">
      <!-- Follow the lift gently; the reduced-motion still has no moving frame. -->
      <animateTransform attributeName="transform" type="translate" dur="4.2s" fill="freeze" calcMode="spline"
        keyTimes="0;.12;.46;.66;1" values="0 -250;0 -250;0 -45;0 0;0 0"
        keySplines="0 0 1 1;.42 0 .35 1;.3 0 .5 1;0 0 1 1"/>
      <g id="gentle-reaction">
      <animateTransform attributeName="transform" type="rotate" dur="4.2s" fill="freeze" calcMode="spline"
        keyTimes="0;.09;.29;.48;.64;.76;.84;.91;1" values="0 527 680;-1.1 527 680;2.3 527 680;-1.6 527 680;.8 527 680;0 527 680;-.7 527 680;.35 527 680;0 527 680"
        keySplines=".42 0 .58 1;.42 0 .45 1;.42 0 .4 1;.42 0 .4 1;.42 0 .4 1;.42 0 .58 1;.3 0 .5 1;.3 0 .5 1"/>
      <g id="stick-insertion">
        <animateTransform attributeName="transform" type="translate" dur="4.2s" fill="freeze" calcMode="spline"
          keyTimes="0;.64;.67;.8;.85;.91;1" values="1260 -220;1260 -220;1080 -160;-26 6;9 -2;-3 1;0 0"
          keySplines="0 0 1 1;.42 0 .58 1;.36 0 .22 1;.3 0 .5 1;.3 0 .5 1;.3 0 .5 1"/>
        <g>
          <animateTransform attributeName="transform" type="rotate" dur="4.2s" fill="freeze" calcMode="spline"
            keyTimes="0;.65;.8;.86;1" values="-13 540 246;-13 540 246;1.3 540 246;-.5 540 246;0 540 246"
            keySplines="0 0 1 1;.42 0 .35 1;.3 0 .5 1;.3 0 .5 1"/>
          <path d="{path(stick)}"/>
        </g>
      </g>
      <g mask="url(#face-clear)">
        <path id="trailing-lock" d="{path(tucked)}">{second_morph}</path>
        <path id="gathering-hair" d="{path(bun)}">{main_morph}</path>
      </g>
      <path id="face" d="{path(head)} {path(opening)} {path(left_eye)} {path(right_eye)}"/>
      </g>
    </g>
    <path class="still" d="{silhouette}"/>
  </g>
</svg>
'''
    (BRAND / "kimo-splash.svg").write_text(svg)
    (ROOT / "web/icons/kimo-splash.svg").write_text(svg)
    preview(svg, "preview.html", "logo motion", "loose hair → bun → hair stick",
            "kimo-splash.svg", DURATION, False,
            [[0, "Loose"], [.4, "Gather"], [1.3, "Twist"], [2.7, "Pin"], [3.65, "Settle"]])
    print(f"Wrote separated artwork and {len(svg.encode()):,}-byte splash SVG (4.2 s, once).")
    build_eyes(outer, opening, left_eye, right_eye)
    build_network(silhouette)


if __name__ == "__main__":
    build()
