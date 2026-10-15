#!/usr/bin/env python3
"""Reckons the colors of codn, from their construction.

A color is three numbers here, in OKLCH: how light it looks (L, from 0 to
1), how far it is from gray (C, its chroma) and its hue (H, an angle). Two
colors of one L look equally light whatever their hue, and two of one C
equally colorful. A hex value says neither.

There are six hues, 60 degrees apart and counted from #00d4d4, which
lies at 195. A step is one L and one C for
every hue it has, so the colors of a step differ in hue alone. Its C is
not more than the screen (sRGB) shows of each of its hues at that L: the
hue with the least room holds the others back. The step of the text lies
where that room is widest.

A gray has the hue 265 and nearly no chroma, most where it is neither
dark nor light: it is said by its L alone. White itself is the gray of
the lightness 1.

To give a hue another step, or a step another hue, add it to STEPS, and a
new gray to GRAYS. Run this and take the line it prints into
src/ui/theme.rs. Tests there hold every color to its place, and no
color comes another way.

usage: python3 scripts/colors.py
"""
import math
import sys

ANCHOR = 195  # the hue of #00d4d4
HUES = {name: (ANCHOR + 60 * turn) % 360 for turn, name in enumerate(("cyan", "blue", "magenta", "red", "amber", "green"))}

# A step: its name, L, C, and the hues that have it.
STEPS = [
    # Text, the marks of a state and the first half of the terminal's table.
    ("text", 0.75, 0.10, ("red", "amber", "green", "cyan", "blue", "magenta")),
    # The bright half of that table.
    ("bright", 0.82, 0.08, ("red", "amber", "green", "cyan", "blue", "magenta")),
    # What warns, and lies under the cross that ends codn: darker, and of a
    # chroma of its own, which the other hues could not follow.
    ("alarm", 0.64, 0.19, ("red",)),
    # Under text that is selected.
    ("selection", 0.35, 0.055, ("blue",)),
    # Under a line that was added or removed.
    ("line", 0.24, 0.035, ("green", "red")),
]

GRAY = 265  # the hue of every gray
# The grays: the name of each and its L.
GRAYS = [
    ("BG", 0.17),  # the ground of a pane
    ("SIDEBAR", 0.20),  # the sidebar, the bar, the tile of the icon
    ("HOVER", 0.24),  # under the pointer, and the header of a pane
    ("LINE", 0.27),  # a line between two parts
    ("ACTIVE", 0.29),  # what is chosen, and black of the terminal
    # Hints, and what need not be read at once: 3 to 1 on the header of a
    # pane. Bright black of the terminal.
    ("FAINT", 0.53),
    ("GROUND", 0.555),  # under a picture, where it is transparent
    ("GROUND_LIGHT", 0.68),  # every other cell of that chequer
    ("DIM", 0.65),  # text that stands back, a comment
    ("WHITE", 0.84),  # white of the terminal
    ("TEXT", 0.885),  # text, the mark of the icon
]


def gray_chroma(l):
    """The chroma of a gray: none at black and white, 0.016 between them."""
    return 0.016 * 4 * l * (1 - l)


def linear(l, c, h):
    """The red, green and blue of a color, as light, before they are bent for the screen."""
    a, b = c * math.cos(math.radians(h)), c * math.sin(math.radians(h))
    long = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3
    middle = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3
    short = (l - 0.0894841775 * a - 1.2914855480 * b) ** 3
    return (
        4.0767416621 * long - 3.3077115913 * middle + 0.2309699292 * short,
        -1.2684380046 * long + 2.6097574011 * middle - 0.3413193965 * short,
        -0.0041960863 * long - 0.7034186147 * middle + 1.7076147010 * short,
    )


def shown(l, c, h):
    """Whether the screen can show the color."""
    return all(-1e-4 <= part <= 1 + 1e-4 for part in linear(l, c, h))


def room(l, h):
    """The largest chroma the screen shows of a hue at a lightness."""
    low, high = 0.0, 0.4
    for _ in range(40):
        middle = (low + high) / 2
        low, high = (middle, high) if shown(l, middle, h) else (low, middle)
    return low


def hex_of(l, c, h):
    def bent(part):
        part = min(1.0, max(0.0, part))
        return round(255 * (12.92 * part if part <= 0.0031308 else 1.055 * part ** (1 / 2.4) - 0.055))

    return "".join(f"{bent(part):02x}" for part in linear(l, c, h))


def main():
    sound = True
    for name, l, c, hues in STEPS:
        least = min(room(l, HUES[hue]) for hue in hues)
        print(f"{name}: L {l}, C {c}, the room of its hues {least:.3f}")
        if c > least:
            sound = False
            print("  more than the screen shows")
        for hue in hues:
            constant = hue.upper() if name == "text" else f"{hue}_{name}".upper()
            print(f"  const {constant}: Color = rgb(0x{hex_of(l, c, HUES[hue])});  // {HUES[hue]}, room {room(l, HUES[hue]):.3f}")
    print(f"gray: the hue {GRAY}")
    for name, l in GRAYS:
        print(f"  const {name}: Color = rgb(0x{hex_of(l, gray_chroma(l), GRAY)});  // L {l}")
    return 0 if sound else 1


if __name__ == "__main__":
    sys.exit(main())
