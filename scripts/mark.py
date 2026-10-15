#!/usr/bin/env python3
"""Draws the codn mark from its construction, and the construction itself.

The mark is built on a grid of equilateral triangles. Its lines run level or
along one slant of 60 degrees, and every corner of the outline is a node of
the grid. A band is two rows high. A corner is either sharp or an arc as wide
as a band, whose centre and both ends are nodes again. The back and the upper
arm, half a band higher, are one flag. The lower arm meets it in a single
node, and there the circle around the grid triangle on either side joins the
two: a third of the large radius, its ends on nodes as well.

To draw another shape the same way, change NODES and nothing else.

usage: uv run --with shapely scripts/mark.py [output directory, default assets]

Writes mark.svg (no background), mark-on-white.svg, mark-on-black.svg,
mark-construction.svg (grid, rows, the circles of every arc), the
application icon: icon.svg, and icon-64.rgba for the window on X11, and
mark.path, the outline alone for the header of the app.
"""
import math
import sys
from pathlib import Path

import numpy
import shapely
from shapely.geometry import Polygon

M = 10  # one module: the side of a grid triangle
ROW = M * math.sqrt(3) / 2  # the height of a grid triangle
BAND = 2 * ROW  # a band is two rows high, and a large arc has this radius
JOINT = M / math.sqrt(3)  # the circle around a grid triangle, a third of BAND

# The outline, node by node: a modules to the right, b rows up along the
# slant, and the radius its corner is rounded with, 0 for a sharp one. The
# back is three modules long and two bands high, the upper arm four modules
# and the lower one five. The node (3, 2) comes twice: there the lower arm
# meets the back.
NODES = [
    (0, 2, 0), (3, 2, JOINT), (3, 0, 0), (8, 0, BAND), (8, 2, 0), (3, 2, JOINT),
    (3, 5, BAND), (7, 5, BAND), (7, 7, 0), (3, 7, 0), (3, 6, BAND), (0, 6, BAND),
]  # fmt: skip

MARGIN = 0.1  # free space around the mark, as a share of the canvas
# The icon: the mark on a tile in the colors of the app, see src/ui/theme.rs.
ICON_MARGIN = 0.18
ICON_CORNER = 0.2  # corner radius of the tile, as a share of its side
ICON_TILE, ICON_MARK = (0x14, 0x16, 0x1B), (0xD7, 0xD9, 0xDD)
ICON_PIXELS = 64
SEGMENTS = 64  # per arc, only for the box of the mark and the pixels of the icon


def place(a, b):
    """Where a node is, y pointing down."""
    return (M * (a + b / 2), -ROW * b)


def corners():
    """Every node with its place and, if it is rounded, its arc: start, end, centre, radius."""
    pts = [place(a, b) for a, b, _ in NODES]
    out = []
    for i, (_, _, r) in enumerate(NODES):
        p, before, after = pts[i], pts[i - 1], pts[(i + 1) % len(pts)]
        if not r:
            out.append((p, None))
            continue
        u, v = (before[0] - p[0], before[1] - p[1]), (after[0] - p[0], after[1] - p[1])
        lu, lv = math.hypot(*u), math.hypot(*v)
        u, v = (u[0] / lu, u[1] / lu), (v[0] / lv, v[1] / lv)
        half = math.acos(max(-1, min(1, u[0] * v[0] + u[1] * v[1]))) / 2
        # how far from the corner the arc touches its two lines
        d = r / math.tan(half)
        assert d <= min(lu, lv) + 1e-9, f"the arc at node {i} does not fit its lines"
        start, end = (p[0] + u[0] * d, p[1] + u[1] * d), (p[0] + v[0] * d, p[1] + v[1] * d)
        w = (u[0] + v[0], u[1] + v[1])
        k = r / math.sin(half) / math.hypot(*w)
        out.append((p, (start, end, (p[0] + w[0] * k, p[1] + w[1] * k), r)))
    return out


def clockwise(start, end, centre):
    return (start[0] - centre[0]) * (end[1] - centre[1]) - (start[1] - centre[1]) * (end[0] - centre[0]) > 0


def contour(dx=0.0, dy=0.0):
    """The exact SVG path, moved by dx and dy, and how many lines it has."""
    d, at, lines = [], None, 0

    def line(to):
        nonlocal at, lines
        if at is None:
            d.append(f"M{to[0] + dx:.4f} {to[1] + dy:.4f}")
        elif math.dist(at, to) > 1e-9:
            d.append(f"L{to[0] + dx:.4f} {to[1] + dy:.4f}")
            lines += 1
        at = to

    for p, arc in corners():
        if not arc:
            line(p)
            continue
        start, end, centre, r = arc
        line(start)
        d.append(f"A{r:.4f} {r:.4f} 0 0 {1 if clockwise(start, end, centre) else 0} {end[0] + dx:.4f} {end[1] + dy:.4f}")
        at = end
    first = corners()[0]
    if math.dist(at, first[1][0] if first[1] else first[0]) > 1e-9:
        lines += 1  # the one Z draws
    return "".join(d) + "Z", lines


def polygon():
    """The outline with its arcs as many short lines."""
    pts = []
    for p, arc in corners():
        if not arc:
            pts.append(p)
            continue
        start, end, centre, r = arc
        a0 = math.atan2(start[1] - centre[1], start[0] - centre[0])
        a1 = math.atan2(end[1] - centre[1], end[0] - centre[0])
        sweep = (a1 - a0 + math.pi) % (2 * math.pi) - math.pi
        pts += [(centre[0] + r * math.cos(a0 + sweep * k / SEGMENTS), centre[1] + r * math.sin(a0 + sweep * k / SEGMENTS)) for k in range(SEGMENTS + 1)]
    shape = Polygon(pts)
    assert shape.is_valid, "the outline crosses itself"
    return shape


def canvas(margin):
    """The square around the mark: left, top, side."""
    x0, y0, x1, y1 = polygon().bounds
    size = max(x1 - x0, y1 - y0) / (1 - 2 * margin)
    return (x0 + x1) / 2 - size / 2, (y0 + y1) / 2 - size / 2, size


def svg_mark(d, fg=None, bg=None):
    x, y, size = canvas(MARGIN)
    back = f'<rect x="{x:g}" y="{y:g}" width="{size:g}" height="{size:g}" fill="{bg}"/>' if bg else ""
    fill = f' fill="{fg}"' if fg else ""
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x:g} {y:g} {size:g} {size:g}">'
        f'{back}<path{fill} d="{d}"/></svg>\n'
    )


def svg_icon(d):
    x, y, size = canvas(ICON_MARGIN)
    tile, mark = "#%02x%02x%02x" % ICON_TILE, "#%02x%02x%02x" % ICON_MARK
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x:g} {y:g} {size:g} {size:g}">'
        f'<rect x="{x:g}" y="{y:g}" width="{size:g}" height="{size:g}" rx="{size * ICON_CORNER:g}" fill="{tile}"/>'
        f'<path fill="{mark}" d="{d}"/></svg>\n'
    )


def icon_pixels(n=ICON_PIXELS, samples=8):
    """The icon as n x n RGBA bytes, straight alpha, each pixel sampled samples x samples times."""
    x, y, size = canvas(ICON_MARGIN)
    step = size / (n * samples)
    at = (numpy.arange(n * samples) + 0.5) * step
    px, py = numpy.meshgrid(x + at, y + at)
    r = size * ICON_CORNER
    # inside the tile: no further than r from the square shrunk by r
    dx = numpy.maximum(numpy.abs(px - (x + size / 2)) - (size / 2 - r), 0)
    dy = numpy.maximum(numpy.abs(py - (y + size / 2)) - (size / 2 - r), 0)
    tile = dx * dx + dy * dy <= r * r
    mark = shapely.contains_xy(polygon(), px, py)

    def cover(mask):
        return mask.reshape(n, samples, n, samples).mean(axis=(1, 3))

    alpha, share = cover(tile), cover(mark)
    out = numpy.zeros((n, n, 4))
    for i in range(3):
        out[..., i] = ICON_TILE[i] + (ICON_MARK[i] - ICON_TILE[i]) * share
    out[..., 3] = 255 * alpha
    return numpy.rint(out).astype(numpy.uint8).tobytes()


def svg_construction(d):
    x0, y0, x1, y1 = polygon().bounds
    # two modules of the grid around the mark
    vx, vy, vw, vh = x0 - 2 * M, y0 - 2 * M, x1 - x0 + 4 * M, y1 - y0 + 4 * M
    line, soft, blue, amber = "#56707c", "#22282d", "#a3cadb", "#e2a85c"
    o = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vx:.3f} {vy:.3f} {vw:.3f} {vh:.3f}">',
        f'<rect x="{vx:.3f}" y="{vy:.3f}" width="{vw:.3f}" height="{vh:.3f}" fill="#0b0c0e"/>',
        '<g fill="none">',
    ]
    far, grid = vw + vh, ""
    for b in range(math.floor(-(vy + vh) / ROW), math.ceil(-vy / ROW) + 1):
        grid += f"M{vx:.2f} {-b * ROW:.3f}H{vx + vw:.2f}"
    for a in range(math.floor((vx - far) / M), math.ceil((vx + vw + far) / M)):
        for lean in (0.5, -0.5):  # both slants through the node (a, 0)
            grid += f"M{a * M - lean * far:.2f} {ROW * far / M:.2f}L{a * M + lean * far:.2f} {-ROW * far / M:.2f}"
    o.append(f'<path d="{grid}" stroke="{soft}" stroke-width=".12"/>')
    rows = "".join(f"M{vx:.2f} {-b * ROW:.3f}H{vx + vw:.2f}" for b in sorted({b for _, b, _ in NODES}))
    o.append(f'<path d="{rows}" stroke="{line}" stroke-width=".16"/>')
    o.append(f'<path d="{d}" fill="#27333a" stroke="{blue}" stroke-width=".28"/>')
    dots = []
    for p, arc in corners():
        if not arc:
            dots.append(f'<circle cx="{p[0]:.3f}" cy="{p[1]:.3f}" r=".5" fill="{blue}"/>')
            continue
        start, end, c, r = arc
        color = blue if r == BAND else amber
        o.append(f'<circle cx="{c[0]:.3f}" cy="{c[1]:.3f}" r="{r:.4f}" stroke="{color}" stroke-width=".16" stroke-dasharray=".9 .9"/>')
        o.append(
            f'<path d="M{start[0]:.3f} {start[1]:.3f}L{c[0]:.3f} {c[1]:.3f}L{end[0]:.3f} {end[1]:.3f}"'
            f' stroke="{color}" stroke-width=".12" opacity=".7"/>'
        )
        dots.append(f'<circle cx="{c[0]:.3f}" cy="{c[1]:.3f}" r=".42" fill="{color}"/>')
        for q in (start, end):
            dots.append(f'<circle cx="{q[0]:.3f}" cy="{q[1]:.3f}" r=".3" fill="#0b0c0e" stroke="{color}" stroke-width=".14"/>')
    o.append("</g>" + "".join(dots) + "</svg>\n")
    return "".join(o)


if __name__ == "__main__":
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "assets")
    out.mkdir(parents=True, exist_ok=True)
    d, lines = contour()
    (out / "mark.svg").write_text(svg_mark(d))
    (out / "mark-on-white.svg").write_text(svg_mark(d, "#000", "#fff"))
    (out / "mark-on-black.svg").write_text(svg_mark(d, "#fff", "#000"))
    (out / "mark-construction.svg").write_text(svg_construction(d))
    (out / "icon.svg").write_text(svg_icon(d))
    (out / f"icon-{ICON_PIXELS}.rgba").write_bytes(icon_pixels())
    x0, y0, x1, y1 = polygon().bounds
    # For src/render/icon.rs: on a square as wide as the mark, in the middle of its height.
    side = x1 - x0
    (out / "mark.path").write_text(contour(-x0, (side - y0 - y1) / 2)[0] + "\n")
    radii = [arc[3] for _, arc in corners() if arc]
    print(f"mark: {side / M:g} modules wide, {(y1 - y0) / ROW:g} rows high, {side:g} x {y1 - y0:.4f}")
    print(f"outline: {lines} lines, {radii.count(BAND)} arcs of a band, {radii.count(JOINT)} of the joint")
