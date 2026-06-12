#!/usr/bin/env python3
"""Generate a vegeta targets file modelling realistic backend load.

Why not hammer one URL: the backend caches built responses per
(milieu, name, lod), so repeating a single URL only measures warm-cache
serving. Real browser load is a *mix of distinct sectors* — the set a
viewport covers, which varies with zoom and map position. parse cost
scales with a sector's world count (~40 sparse rim vs ~440 dense core),
so the mix must span both.

This mirrors the frontend's `render::visible_sectors`: a viewport at
`scale` px/parsec over a WxH canvas spans (W/scale)x(H/scale) parsecs;
in sectors that's ~(W/scale/32) x (H/scale/40). We pick representative
(center, zoom) viewports, select every indexed sector whose grid cell
falls in that window, union across viewports, and emit one target per
distinct sector. The frontend only ever requests lod=overview, so we do
too. (MAX_STREAM=48 caps a single viewport's fetch fan-out.)

Usage:
    python3 loadtest/gen_targets.py > loadtest/targets.txt
"""
import json
import sys
import urllib.parse
import urllib.request

BASE = "http://127.0.0.1:3000"
MILIEU = "M1105"
MAX_STREAM = 48           # frontend cap per viewport
CANVAS_W, CANVAS_H = 1400, 950   # a typical full-window canvas (logical px)
SECTOR_W, SECTOR_H = 32, 40      # parsecs per sector

# (center sector name, scale px/parsec) viewports. Scales chosen across the
# render LOD band: 48=full detail (~1 sector wide), 24=poster (~2), 8=just
# above WORLD_MIN_SCALE=4 (~5-6 sectors, the busiest fetch). Centers span
# dense core (Core, Vland, Massilia) and sparser frontier (Spinward Marches,
# Trojan Reach, The Beyond) so parse cost varies.
VIEWPORTS = [
    ("Spinward Marches", 48),
    ("Spinward Marches", 24),
    ("Spinward Marches", 8),
    ("Core", 24),
    ("Core", 8),
    ("Vland", 12),
    ("Massilia", 8),
    ("Trojan Reach", 24),
    ("The Beyond", 12),
    ("Solomani Rim", 8),
    ("Deneb", 12),
    ("Gvurrdon", 16),
]


def fetch_universe():
    url = f"{BASE}/api/universe?milieu={MILIEU}"
    with urllib.request.urlopen(url, timeout=10) as r:
        return json.load(r)


def main():
    uni = fetch_universe()
    by_name = {s["name"]: (s["location"]["x"], s["location"]["y"]) for s in uni["sectors"]}
    by_cell = {(x, y): name for name, (x, y) in by_name.items()}

    selected = set()
    for center_name, scale in VIEWPORTS:
        if center_name not in by_name:
            print(f"# WARN: center '{center_name}' not in index, skipped", file=sys.stderr)
            continue
        cx, cy = by_name[center_name]
        # half-window in sectors (ceil), matching visible_sectors' span.
        half_sx = int((CANVAS_W / scale) / SECTOR_W / 2) + 1
        half_sy = int((CANVAS_H / scale) / SECTOR_H / 2) + 1
        window = []
        for dx in range(-half_sx, half_sx + 1):
            for dy in range(-half_sy, half_sy + 1):
                cell = (cx + dx, cy + dy)
                if cell in by_cell:
                    window.append(by_cell[cell])
        # honor the frontend's per-viewport fetch cap
        for name in window[:MAX_STREAM]:
            selected.add(name)
        print(
            f"# {center_name} @ {scale}px/pc -> {min(len(window), MAX_STREAM)} sectors "
            f"(window had {len(window)})",
            file=sys.stderr,
        )

    for name in sorted(selected):
        enc = urllib.parse.quote(name)
        print(f"GET {BASE}/api/sector/{MILIEU}/{enc}?lod=overview")

    print(f"# {len(selected)} distinct sector targets", file=sys.stderr)


if __name__ == "__main__":
    main()
