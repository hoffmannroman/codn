#!/usr/bin/env python3
"""Draws the logo of codn, the mark with the word codn beside it, and its construction.

The word is set in Cal Sans on the grid of the mark, in the measures of
scripts/mark.py: a module is the side of a grid triangle, a row its height.
The baseline is row 0, the foot of the mark. The x-height is row 5, the
underside of the upper arm. The top of the d is row 7, the top of the mark.
A stem is one module wide. The leftmost point of the c lies three modules
right of the tip of the lower arm, which is the length of the back.

Cal Sans is a variable font, so no letter is redrawn for this: its weight is
solved for the stem, and the height of its ascenders for the d. The letters,
how far the round ones reach past a row, and their spacing stay the font's.

usage: uv run --with shapely --with fonttools --with uharfbuzz scripts/logo.py [output directory, default assets]

Writes logo.svg (no background), logo-white.svg (the same in white, for a dark
page), logo-on-white.svg, logo-on-black.svg and logo-construction.svg (the grid,
the three rows of the word, its measures).
"""
import importlib.util
import math
import sys
from pathlib import Path

import uharfbuzz as hb
from fontTools.pens.basePen import BasePen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from shapely.geometry import LineString, Polygon

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("mark", HERE / "mark.py")
mark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mark)
M, ROW, BAND = mark.M, mark.ROW, mark.BAND

FONT = HERE.parent / "assets" / "fonts" / "CalSansVF.ttf"
WORD = "codn"
X_ROWS, TOP_ROWS = 5, 7  # the rows of the x-height and of the top of the d
STEM = 1  # modules
GAP, TIP = 3, (8, 2)  # modules from this node of the mark to the leftmost point of the word
FREE = 2  # modules left free around the logo
# Where on the axes of the font the word is taken, besides the two that are solved: the drawing for
# large sizes, the standard family, whose c ends in a slant, upright, the corners as they are drawn.
PLACE = {"opsz": 45, "GEOM": 50, "SHRP": 0, "ital": 0}
WEIGHTS, ASCENDERS = (400.0, 700.0), (1440.0, 1600.0)  # what the two solved axes can give
# The words on the drawing of the construction are in the same font, small and regular.
LABEL = {"opsz": 14, "GEOM": 50, "wght": 400, "YTAS": 1440, "SHRP": 0, "ital": 0}
STEPS = 64  # per curve, only for measuring

face = hb.Face(hb.Blob.from_file_path(str(FONT)))


class Flat(BasePen):
    """Collects the contours of a glyph as rings of points, its curves as many short lines."""

    def __init__(self):
        super().__init__(None)
        self.rings, self.ring = [], []

    def _moveTo(self, p):
        self.ring = [p]

    def _lineTo(self, p):
        self.ring.append(p)

    def _curveToOne(self, a, b, c):
        p = self.ring[-1]
        for i in range(1, STEPS + 1):
            t, u = i / STEPS, 1 - i / STEPS
            self.ring.append(
                tuple(u**3 * p[k] + 3 * u * u * t * a[k] + 3 * u * t * t * b[k] + t**3 * c[k] for k in (0, 1))
            )

    def _qCurveToOne(self, a, b):
        p = self.ring[-1]
        for i in range(1, STEPS + 1):
            t, u = i / STEPS, 1 - i / STEPS
            self.ring.append(tuple(u * u * p[k] + 2 * u * t * a[k] + t * t * b[k] for k in (0, 1)))

    def _closePath(self):
        if len(self.ring) > 2:
            self.rings.append(self.ring)
        self.ring = []

    _endPath = _closePath


def shaped(text, axes):
    """The font at that place, the glyphs of the text with where each begins, and where the text ends."""
    font = hb.Font(face)
    font.set_variations(axes)
    buf = hb.Buffer()
    buf.add_str(text)
    buf.guess_segment_properties()
    hb.shape(font, buf)
    glyphs, x = [], 0
    for info, pos in zip(buf.glyph_infos, buf.glyph_positions):
        assert info.codepoint, f"the font has no glyph for a letter of {text!r}"
        glyphs.append((info.codepoint, x + pos.x_offset))
        x += pos.x_advance
    return font, glyphs, x


def shapes(text, axes):
    """Every glyph of the text as a polygon where it stands, in the units of the font."""
    font, glyphs, _ = shaped(text, axes)
    out = []
    for glyph, x in glyphs:
        pen = Flat()
        font.draw_glyph_with_pen(glyph, pen)
        shape = None
        for ring in pen.rings:  # a contour inside another is a hole
            part = Polygon([(px + x, py) for px, py in ring]).buffer(0)
            shape = part if shape is None else shape.symmetric_difference(part)
        out.append(shape)
    return out


def across(shape, y):
    """The first stretch of a level line at y that lies inside the shape: where it begins and ends."""
    x0, _, x1, _ = shape.bounds
    got = shape.intersection(LineString([(x0 - 10, y), (x1 + 10, y)]))
    return min((g.bounds[0], g.bounds[2]) for g in getattr(got, "geoms", [got]) if not g.is_empty)


def measures(axes):
    """What the word measures at that place, in the units of the font."""
    xh = shapes("x", axes)[0].bounds[3]  # the x is flat on top, the round letters reach past it
    c, o, d, n = shapes(WORD, axes)
    return {
        "xh": xh,
        "top": d.bounds[3],
        "left": c.bounds[0],
        "right": n.bounds[2],
        "over": (o.bounds[3], o.bounds[1]),
        "round": (c.bounds[0], o.bounds[2]),
        "stem_d": across(d, (xh + d.bounds[3]) / 2),
        "stem_n": across(n, xh / 4),
    }


def stem(weight):
    """The width of a stem at that weight, the mean of the d and the n, as a share of the x-height."""
    m = measures({**PLACE, "wght": weight, "YTAS": ASCENDERS[0]})
    return (m["stem_d"][1] - m["stem_d"][0] + m["stem_n"][1] - m["stem_n"][0]) / 2 / m["xh"]


def solve():
    """The place on the axes at which a stem is a module and the d seven rows, the x-height being five."""
    want = STEM * M / (X_ROWS * ROW)
    lo, hi = WEIGHTS
    assert stem(lo) < want < stem(hi), "no weight of the font gives a stem of that width"
    for _ in range(40):
        mid = (lo + hi) / 2
        lo, hi = (mid, hi) if stem(mid) < want else (lo, mid)
    weight = (lo + hi) / 2
    # The axis of the ascenders is the height of the d itself, in the units of the font.
    top = measures({**PLACE, "wght": weight, "YTAS": ASCENDERS[0]})["xh"] * TOP_ROWS / X_ROWS
    assert ASCENDERS[0] <= top <= ASCENDERS[1], "the font has no ascender of that height"
    return {**PLACE, "wght": weight, "YTAS": top}


AXES = solve()
MEASURED = measures(AXES)
SCALE = X_ROWS * ROW / MEASURED["xh"]  # units of the mark for one unit of the font
LEFT = mark.place(*TIP)[0] + GAP * M - MEASURED["left"] * SCALE  # where the word begins on the page


def page(x):
    """Where a place across the word, in the units of the font, lies on the page."""
    return LEFT + x * SCALE


def outline(text, axes, scale, x, y):
    """The text as one SVG path with the true curves of the font, beginning at x on the baseline y."""
    font, glyphs, _ = shaped(text, axes)
    pen = SVGPathPen(None, ntos=lambda v: f"{v:.3f}".rstrip("0").rstrip("."))
    for glyph, gx in glyphs:
        font.draw_glyph_with_pen(glyph, TransformPen(pen, (scale, 0, 0, -scale, x + gx * scale, y)))
    return pen.getCommands()


def box():
    """The logo without free space: left, top, width, height. The mark is its height."""
    x0, y0, _, y1 = mark.polygon().bounds
    return x0, y0, page(MEASURED["right"]) - x0, y1 - y0


def svg_logo(d, word, fg=None, bg=None):
    x, y, w, h = box()
    x, y, w, h = x - FREE * M, y - FREE * M, w + 2 * FREE * M, h + 2 * FREE * M
    back = f'<rect x="{x:.3f}" y="{y:.3f}" width="{w:.3f}" height="{h:.3f}" fill="{bg}"/>' if bg else ""
    fill = f' fill="{fg}"' if fg else ""
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{x:.3f} {y:.3f} {w:.3f} {h:.3f}">'
        f'{back}<path{fill} d="{d}"/><path{fill} d="{word}"/></svg>\n'
    )


def svg_construction(d, word):
    x0, y0, w, h = box()
    x1 = mark.polygon().bounds[2]
    # three modules of the grid around the logo, and room at the right for the names of the rows
    vx, vy, vw, vh = x0 - 3 * M, y0 - 3.4 * M, w + 7.5 * M, h + 6.4 * M
    line, soft, blue, amber, ground = "#56707c", "#22282d", "#a3cadb", "#e2a85c", "#0b0c0e"
    o = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vx:.3f} {vy:.3f} {vw:.3f} {vh:.3f}">',
        f'<rect x="{vx:.3f}" y="{vy:.3f}" width="{vw:.3f}" height="{vh:.3f}" fill="{ground}"/>',
        '<g fill="none">',
    ]
    far, grid = vw + vh, ""
    for b in range(math.floor(-(vy + vh) / ROW), math.ceil(-vy / ROW) + 1):
        grid += f"M{vx:.2f} {-b * ROW:.3f}H{vx + vw:.2f}"
    for a in range(math.floor((vx - far) / M), math.ceil((vx + vw + far) / M)):
        for lean in (0.5, -0.5):  # both slants through the node (a, 0)
            grid += f"M{a * M - lean * far:.2f} {ROW * far / M:.2f}L{a * M + lean * far:.2f} {-ROW * far / M:.2f}"
    o.append(f'<path d="{grid}" stroke="{soft}" stroke-width=".12"/>')
    rows = "".join(f"M{vx:.2f} {-b * ROW:.3f}H{vx + vw:.2f}" for b in sorted({b for _, b, _ in mark.NODES}))
    o.append(f'<path d="{rows}" stroke="{line}" stroke-width=".16"/>')
    # the three rows the word stands on, brighter, from the mark to the right edge
    three = "".join(f"M{x1 + M:.2f} {-b * ROW:.3f}H{vx + vw:.2f}" for b in (0, X_ROWS, TOP_ROWS))
    o.append(f'<path d="{three}" stroke="{blue}" stroke-width=".2"/>')
    o.append(f'<path d="{d}" fill="#27333a" stroke="{blue}" stroke-width=".28"/>')
    o.append(f'<path d="{word}" fill="#27333a" stroke="{blue}" stroke-width=".28"/>')
    dots, words = [], []
    for p, arc in mark.corners():
        if not arc:
            dots.append(f'<circle cx="{p[0]:.3f}" cy="{p[1]:.3f}" r=".5" fill="{blue}"/>')
            continue
        start, end, c, r = arc
        color = blue if r == BAND else amber
        o.append(
            f'<circle cx="{c[0]:.3f}" cy="{c[1]:.3f}" r="{r:.4f}" stroke="{color}" stroke-width=".16"'
            ' stroke-dasharray=".9 .9"/>'
        )
        o.append(
            f'<path d="M{start[0]:.3f} {start[1]:.3f}L{c[0]:.3f} {c[1]:.3f}L{end[0]:.3f} {end[1]:.3f}"'
            f' stroke="{color}" stroke-width=".12" opacity=".7"/>'
        )
        dots.append(f'<circle cx="{c[0]:.3f}" cy="{c[1]:.3f}" r=".42" fill="{color}"/>')
        for q in (start, end):
            dots.append(
                f'<circle cx="{q[0]:.3f}" cy="{q[1]:.3f}" r=".3" fill="{ground}" stroke="{color}" stroke-width=".14"/>'
            )

    def label(x, y, text, color, anchor):
        """Words on the drawing, as outlines, so that it needs no font where it is shown."""
        scale = 2.6 / face.upem
        width = shaped(text, LABEL)[2] * scale
        x -= {"start": 0, "middle": width / 2, "end": width}[anchor]
        words.append(f'<path d="{outline(text, LABEL, scale, x, y)}" fill="{color}"/>')

    def span(xa, xb, y, text, above):
        """A measure between two places across, with its ticks and its words."""
        o.append(
            f'<path d="M{xa:.3f} {y:.3f}H{xb:.3f}M{xa:.3f} {y - 1.1:.3f}V{y + 1.1:.3f}M{xb:.3f} {y - 1.1:.3f}V{y + 1.1:.3f}"'
            f' stroke="{amber}" stroke-width=".16"/>'
        )
        label((xa + xb) / 2, y - 1.7 if above else y + 3.5, text, amber, "middle")

    edge = vx + vw - 0.8 * M
    for row, name in ((TOP_ROWS, "top of the d"), (X_ROWS, "x-height"), (0, "baseline")):
        label(edge, -row * ROW - 1.1, f"row {row} · {name}", blue, "end")
    # from the tip of the lower arm to the c, node by node along the row of the tip
    below = 1.9 * M
    tip = mark.place(*TIP)
    at = tip[0] + GAP * M
    o.append(
        f'<path d="M{at:.3f} {ROW:.3f}V{-(X_ROWS + 1) * ROW:.3f}" stroke="{amber}" stroke-width=".14"'
        ' stroke-dasharray=".9 .9"/>'
    )
    o.append(f'<path d="M{tip[0]:.3f} {tip[1]:.3f}V{below:.3f}" stroke="{amber}" stroke-width=".12" opacity=".7"/>')
    span(tip[0], at, below, f"{GAP} modules", above=False)
    for k in range(GAP + 1):
        dots.append(
            f'<circle cx="{tip[0] + k * M:.3f}" cy="{tip[1]:.3f}" r=".3" fill="{ground}" stroke="{amber}"'
            ' stroke-width=".14"/>'
        )
    # a stem, as it is measured: on the d above the word, on the n below it
    above = -TOP_ROWS * ROW - 1.6 * M
    for (a, b), y0, y1 in ((MEASURED["stem_d"], -TOP_ROWS * ROW, above), (MEASURED["stem_n"], 0, below)):
        xa, xb = page(a), page(b)
        o.append(
            f'<path d="M{xa:.3f} {y0:.3f}V{y1:.3f}M{xb:.3f} {y0:.3f}V{y1:.3f}" stroke="{amber}" stroke-width=".12"'
            ' opacity=".7"/>'
        )
        span(xa, xb, y1, f"{(xb - xa) / M:.2f} module", above=y1 < 0)
    # how far the round letters reach past the two rows, which is the font's and no rule of the grid
    top, foot = (-v * SCALE for v in MEASURED["over"])
    xa, xb = page(MEASURED["round"][0]) - M, page(MEASURED["round"][1]) + M
    o.append(
        f'<path d="M{xa:.3f} {top:.3f}H{xb:.3f}M{xa:.3f} {foot:.3f}H{xb:.3f}" stroke="{line}" stroke-width=".12"'
        ' stroke-dasharray=".5 .5"/>'
    )
    o.append("</g>" + "".join(dots) + "".join(words) + "</svg>\n")
    return "".join(o)


if __name__ == "__main__":
    out = Path(sys.argv[1] if len(sys.argv) > 1 else HERE.parent / "assets")
    out.mkdir(parents=True, exist_ok=True)
    d, _ = mark.contour()
    word = outline(WORD, AXES, SCALE, LEFT, 0)
    (out / "logo.svg").write_text(svg_logo(d, word))
    (out / "logo-white.svg").write_text(svg_logo(d, word, "#fff"))
    (out / "logo-on-white.svg").write_text(svg_logo(d, word, "#000", "#fff"))
    (out / "logo-on-black.svg").write_text(svg_logo(d, word, "#fff", "#000"))
    (out / "logo-construction.svg").write_text(svg_construction(d, word))
    m, modules = MEASURED, SCALE / M
    _, _, w, h = box()
    print(f"font: wght {AXES['wght']:.1f}, YTAS {AXES['YTAS']:.1f}, opsz {PLACE['opsz']}, GEOM {PLACE['GEOM']}")
    print(f"word: x-height {m['xh'] * SCALE / ROW:.3f} rows, top of the d {m['top'] * SCALE / ROW:.3f} rows")
    print(
        f"stems: d {(m['stem_d'][1] - m['stem_d'][0]) * modules:.3f}, n {(m['stem_n'][1] - m['stem_n'][0]) * modules:.3f}"
        f" modules; round letters reach {(m['over'][0] - m['xh']) * modules:.3f} above and {-m['over'][1] * modules:.3f} below"
    )
    print(f"logo: {w / M:.3f} modules wide, {h / ROW:g} rows high, the word {(m['right'] - m['left']) * modules:.3f} wide")
