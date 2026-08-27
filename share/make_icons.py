#!/usr/bin/env python3
"""
Generate the tray icons used by moonwatch_rs, and the Windows icon of the executable.

The icons are embedded into the binary - the .png files by src/daemon/tray.rs, the .ico as a
Windows resource by build.rs - so they are checked in; run this only when you want to change
how they look.

    pip install pillow
    ./make_icons.py

(or, without a system Python: `uv run --with pillow share/make_icons.py`)

Each icon is a crescent moon drawn at 8x and downscaled, giving antialiased edges without
needing a vector library. There are three, matching MoonwatcherStatus:

    moonwatch-icon.png         amber      recording
    moonwatch-icon-paused.png  grey       recording paused
    moonwatch-icon-error.png   grey + red configuration problem

The dark rim keeps the moon visible on light panels (Windows light theme), and the error
badge is a plain disc rather than an exclamation mark because at the 16x16 the shell
actually displays, any glyph inside it turns to mush.

The amber icon is additionally written as

    moonwatch-icon.ico         the icon Explorer and the taskbar show for the .exe

with every size the shell asks for, each downscaled from the same 8x master rather than from
the 64x64 .png, so none of them is an upscale.
"""

import os.path as op
from PIL import Image, ImageDraw, ImageFilter

SIZE = 64
SUPERSAMPLE = 8
N = SIZE * SUPERSAMPLE

# What Windows picks from: 16 in Explorer's details view, 32 in the taskbar and Alt-Tab,
# 256 in the extra-large view and the file properties dialog, the rest for the sizes in
# between and for high-DPI displays.
ICO_SIZES = (16, 24, 32, 48, 64, 128, 256)

AMBER = (255, 201, 77, 255)
AMBER_RIM = (46, 34, 8, 235)
GREY = (150, 155, 165, 255)
GREY_RIM = (38, 40, 46, 225)
BADGE = (226, 55, 45, 255)
BADGE_RIM = (255, 255, 255, 235)

OUT_DIR = op.abspath(op.dirname(__file__))


def disc(cx, cy, r):
    """Antialiasing-friendly filled circle as an L mask, in supersampled coordinates."""
    mask = Image.new("L", (N, N), 0)
    ImageDraw.Draw(mask).ellipse([cx - r, cy - r, cx + r, cy + r], fill=255)
    return mask


def subtract(mask, hole):
    return Image.composite(Image.new("L", (N, N), 0), mask, hole)


def grow(mask, pixels):
    return mask.filter(ImageFilter.MaxFilter(2 * pixels * SUPERSAMPLE + 1))


def draw_icon(fill, rim, badge=False):
    """The icon at the supersampled size, for the caller to downscale as it needs."""
    # Crescent = big disc with a smaller offset disc taken out of it.
    crescent = subtract(disc(N * 0.46, N * 0.52, N * 0.40),
                        disc(N * 0.70, N * 0.34, N * 0.36))
    outline = subtract(grow(crescent, 1), crescent)

    img = Image.new("RGBA", (N, N), (0, 0, 0, 0))
    img.paste(rim, (0, 0), outline)
    img.paste(fill, (0, 0), crescent)

    if badge:
        dot = disc(N * 0.76, N * 0.78, N * 0.19)
        img.paste(BADGE_RIM, (0, 0), subtract(grow(dot, 1), dot))
        img.paste(BADGE, (0, 0), dot)

    return img


def make_icon(filename, fill, rim, badge=False):
    master = draw_icon(fill, rim, badge)

    path = op.join(OUT_DIR, filename)
    master.resize((SIZE, SIZE), Image.LANCZOS).save(path)
    print("wrote", path)

    return master


def make_ico(filename, master):
    """Write the Windows executable icon, holding every size the shell asks for."""
    path = op.join(OUT_DIR, filename)
    # Pillow resizes the image it is given for each entry, so it gets the 8x master. Every
    # entry comes out PNG-compressed rather than as a BMP; Windows has read those at any
    # size since 7, and it keeps the file to a few tens of kilobytes.
    master.save(path, sizes=[(size, size) for size in ICO_SIZES])
    print("wrote", path, "with sizes", ICO_SIZES)


if __name__ == "__main__":
    amber = make_icon("moonwatch-icon.png", AMBER, AMBER_RIM)
    make_icon("moonwatch-icon-paused.png", GREY, GREY_RIM)
    make_icon("moonwatch-icon-error.png", GREY, GREY_RIM, badge=True)

    make_ico("moonwatch-icon.ico", amber)
