#!/usr/bin/env python3
"""Write background.svg: the artwork of the DMG window, with its text turned into outlines.

Only needed to change the artwork. Needs `pip install fonttools` and reads the fonts the repository
already ships (assets/fonts, SIL OFL). The outlined text means `background.svg` renders the same on
every machine with no fonts installed.

    python3 packaging/macos/dmg/make_background.py
    python3 packaging/macos/dmg/generate.py        # then renders background.tiff and the layout
"""

import os

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

HERE = os.path.dirname(os.path.abspath(__file__))
FONTS = os.path.join(HERE, "..", "..", "..", "assets", "fonts")

INK = "#0b0f24"
PAPER = "#eef1f8"
CYAN = "#1f7bff"
MAGENTA = "#a23bff"


def outline(font_file, text, size, x, y, spacing=0.0):
    """The path data of `text` set in `font_file` at `size` px, its baseline at y, starting at x."""
    font = TTFont(os.path.join(FONTS, font_file))
    glyphs, cmap, hmtx = font.getGlyphSet(), font.getBestCmap(), font["hmtx"]
    scale = size / font["head"].unitsPerEm
    pen_x, d = x, []
    for ch in text:
        name = cmap[ord(ch)]
        path = SVGPathPen(glyphs, ntos=lambda v: f"{v:.2f}".rstrip("0").rstrip("."))
        glyphs[name].draw(TransformPen(path, (scale, 0, 0, -scale, pen_x, y)))
        d.append(path.getCommands())
        pen_x += hmtx[name][0] * scale + spacing
    return " ".join(p for p in d if p)


def text(font_file, content, size, x, y, fill, spacing=0.0):
    return f'<path fill="{fill}" d="{outline(font_file, content, size, x, y, spacing)}"/>'


svg = f"""<svg xmlns="http://www.w3.org/2000/svg" width="660" height="400" viewBox="0 0 660 400">
<defs>
  <clipPath id="panel"><rect width="232" height="400"/></clipPath>
  <linearGradient id="night" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#1b0f45"/><stop offset="1" stop-color="#060514"/></linearGradient>
  <linearGradient id="edge" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#4af2ff"/><stop offset="1" stop-color="#ff5ce1"/></linearGradient>
</defs>

<rect width="660" height="400" fill="{PAPER}"/>
<g clip-path="url(#panel)">
  <rect width="232" height="400" fill="url(#night)"/>
  <g transform="translate(-114,-10) scale(0.9)"><image href="../../../assets/app-icon/xmlcraft.svg" width="512" height="512"/></g>
</g>
<rect x="232" y="0" width="3" height="400" fill="url(#edge)"/>

{text("Inter-SemiBold.ttf", "XmlCraft", 38, 266, 72, INK, -0.6)}
<line x1="267" y1="88.5" x2="624" y2="88.5" stroke="{INK}" stroke-width="0.6"/>
<path d="M271.5,81.5 L276,88.5 L271.5,95.5 L267,88.5 Z" fill="{CYAN}"/>
{text("JetBrainsMono-Regular.ttf", "XML · XSD · DTD · validate · generate", 12, 267, 114, "#4a5068")}

<rect x="408" y="197" width="78" height="16" rx="2" fill="{CYAN}" fill-opacity="0.35"/>
<line x1="414" y1="205" x2="482" y2="205" stroke="{INK}" stroke-width="1.6"/>
<path d="M499.1,205.0 L486.1,211.0 L486.1,199.0 Z" fill="{INK}"/>
<line x1="267" y1="335.5" x2="624" y2="335.5" stroke="{INK}" stroke-width="0.6"/>
{text("Inter-Medium.ttf", "DRAG ONTO APPLICATIONS TO INSTALL", 12, 267, 358, "#4a5068", 1.8)}
</svg>
"""

with open(os.path.join(HERE, "background.svg"), "w", encoding="utf-8") as f:
    f.write(svg)
