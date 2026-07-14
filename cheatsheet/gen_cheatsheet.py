#!/usr/bin/env python3
"""Generate the midip cheat sheet.

Single source of truth for `midip-cheatsheet.pdf` (the repo root file the README
links to). Edit CONTENT below and run:

    python3 cheatsheet/gen_cheatsheet.py

It writes `cheatsheet/midip-cheatsheet.svg` and, if `rsvg-convert` is on PATH, renders
`midip-cheatsheet.pdf` at the repo root (A4 landscape). Install rsvg-convert via
`brew install librsvg` (macOS) or `apt install librsvg2-bin` (Debian/Ubuntu).

Style: white page, large title, subtitle + rule, three columns of gray-barred
sections. Keep KEEP `VERSION` in step with Cargo.toml on each release.
"""

import os
import shutil
import subprocess
import sys

# A4 landscape, in points (matches the historical page size).
W, H = 841.89, 595.28
MARGIN = 28.0
GAP = 14.0
COLW = (W - 2 * MARGIN - 2 * GAP) / 3
COLX = [MARGIN, MARGIN + COLW + GAP, MARGIN + 2 * (COLW + GAP)]

# Vertical rhythm.
TITLE_Y = 46
SUB_Y = 62
RULE_Y = 74
BODY_TOP = 92
HDR_H = 13.5       # header bar height
HDR_PAD_BELOW = 4  # gap between a header bar and its first body line
LINE_H = 10.2
SECTION_GAP = 7.5

# Colors (sampled from the original PDF).
FG = "#333333"
SUB = "#777777"
HDR_BG = "#ececec"
RULE = "#cccccc"

FONT = "Helvetica, Arial, sans-serif"

VERSION = "v1.4.1"

# Three columns, each a list of (TITLE, [lines]).
CONTENT = [
    # ---- Column 1 ----
    [
        ("DEVICES  [d] PICKER", [
            "Drums: T-8 · RD-8 · DrumBrute · Circuit Drum · GM*",
            "Synth: S-1 · J-6 · TD-3 · monologue · MicroFreak",
            "       minilogue xd · Digitakt · Circuit Synth",
            "[d] assign per lane (kind-filtered) + re-route",
            "* generic fallback · add your own via devices.json",
        ]),
        ("TRANSPORT", [
            "[space] play / stop",
            "[esc] panic / all-notes-off   [!] full panic",
            "[t] type BPM  [T] tap tempo  [k] Link",
            "[; / '] BPM −/+   [< / >] swing",
            "[{ / }] pattern len   [L] double length",
        ]),
        ("EDIT", [
            "[tab / shift+tab] next / prev lane",
            "[enter] toggle step / place note   [del] clear",
            "[0-9] velocity bucket   [+ / −] fine vel",
            "[p / P] probability   [y / Y] ratchet",
            "[x c v] cut / copy / paste   [r / R] rotate",
        ]),
        ("MOUSE  (EDIT)", [
            "[click] toggle step · move cursor",
            "[drag] paint hits across cells (drums)",
            "[scroll] velocity of step under pointer",
            "MIDIP_MOUSE=0 disable (keep text selection)",
        ]),
        ("DRUMS", [
            "[← →] step   [↑ ↓] voice row",
            "[e / E] euclid pulses   [[ / ]] rotation",
            "[`] toggle voice mute on cursor row",
        ]),
        ("MELODIC", [
            "[← →] step   [↑ ↓] pitch",
            "[g] slide   [, / .] note len   [[ / ]] octave",
        ]),
        ("CHORDS  (POLY LANES)", [
            "[j] build scale-aware triad   [J] remove note",
            "note-input STACKS keys into a chord",
        ]),
        ("SCALES  (MELODIC)", [
            "[n / N] cycle scale   [h / H] root −/+",
            "[X] conform to scale   [I] note-input",
        ]),
    ],
    # ---- Column 2 ----
    [
        ("NOTE INPUT  [I]", [
            "[a s d f g h j k] white   [w e t y u] black",
            "[z / x] octave   [Bksp] clear   [Esc] exit",
        ]),
        ("PER-STEP", [
            "[\\ / |] micro timing −/+",
            "[z] cycle trig cond (1:2 / 1:3 / Fill / …)",
            "[@ / #] add / remove CC   [$ / ^] CC val +/−",
        ]),
        ("PER-LANE", [
            "[a / _] lane swing override −/+",
            "[Q] cycle clock divisor (/1 /2 /3 /4)",
        ]),
        ("GLOBAL", [
            "[ctrl+z] / [u] undo   [ctrl+y / ctrl+r] redo",
            "[m] mute  [S] solo  [M] mirror output",
            "[l] library  [o] open set  [s] save",
            "[?] help  [q] quit (twice while playing)",
        ]),
        ("ROUTING / PERFORMANCE", [
            "[w] route editor   [d] device picker",
            "[W] clock-in source selector",
            "[b] launch quant   [C] cancel queued launch",
            "[i] restart lane phase   [f]/[F] fill / commit",
        ]),
        ("ROUTE EDITOR  [w]", [
            "[↑ ↓] lane   [← →] field",
            "[c / C] cycle port   [[ / ]] channel (1-16)",
            "[z] toggle MIDI clock out   [esc] close",
        ]),
        ("LIBRARY  [l]", [
            "[enter] load (queues if playing)",
            "[a] audition   [esc / l] close",
        ]),
        ("SET MANAGER  [o]", [
            "[enter] load   [r] rename   [a / S] save-as",
            "[D] duplicate  [d] delete   [n] new set",
        ]),
    ],
    # ---- Column 3 ----
    [
        ("PATTERN  (EDIT)", [
            "[A] save lane as user pattern   [Z] clear lane",
        ]),
        ("OVERVIEW / DISPLAY", [
            "grid marker: ² ³ ratchet · ° chance",
            "             ? trig-cond · ≈ microtiming",
            "'acc' row: per-step accent (loudest voice)",
            "LANES meter: 8-bin density sparkline",
            "MIDIP_ASCII=1 ASCII-only glyphs",
        ]),
        ("CRATE / LIVE  [V]", [
            "[↑ ↓] entry   [← →] crate   [enter] launch",
            "[a] audition   [z] validate   [V / esc] close",
        ]),
        ("SCENES  [G]", [
            "[↑ ↓] select   [enter] recall   [c] capture",
            "[r] rename  [d] dup  [x] del  [G/esc] close",
        ]),
        ("CHAINS  [K]", [
            "[enter] play   [C] stop   [j] live jump",
            "[c] create   [a] add scene   [X] remove entry",
            "[[ / ]] bars   [{ / }] repeats   [K/esc] close",
        ]),
        ("GENERATIVE  [D]", [
            "[tab] vary   [shift+tab] generate",
            "[d / D] density  [r / R] range  [m / M] mutate",
            "[z] reroll seed   [enter] commit   [esc] cancel",
        ]),
        ("FAVORITES  (LIBRARY)", [
            "[f] toggle favorite   [F] favorites-only filter",
        ]),
        ("CLOCK-IN  [W]", [
            "[↑ ↓] input port   [enter] confirm   [esc] cancel",
        ]),
    ],
]


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def build_svg():
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" '
        f'width="{W}pt" height="{H}pt" viewBox="0 0 {W} {H}">',
        f'<rect x="0" y="0" width="{W}" height="{H}" fill="white"/>',
        f'<text x="{MARGIN}" y="{TITLE_Y}" font-family="{FONT}" '
        f'font-size="26" fill="{FG}">midip  —  cheat sheet</text>',
        f'<text x="{MARGIN}" y="{SUB_Y}" font-family="{FONT}" font-size="9" '
        f'fill="{SUB}">Terminal MIDI sequencer &amp; live groovebox  ·  '
        f'{VERSION}  ·  github.com/gdamdam/midip</text>',
        f'<line x1="{MARGIN}" y1="{RULE_Y}" x2="{W - MARGIN}" y2="{RULE_Y}" '
        f'stroke="{RULE}" stroke-width="0.8"/>',
    ]
    for ci, sections in enumerate(CONTENT):
        x = COLX[ci]
        y = BODY_TOP
        for title, lines in sections:
            out.append(
                f'<rect x="{x}" y="{y}" width="{COLW}" height="{HDR_H}" '
                f'fill="{HDR_BG}"/>'
            )
            out.append(
                f'<text x="{x + 4}" y="{y + HDR_H - 4}" font-family="{FONT}" '
                f'font-size="8.5" font-weight="bold" fill="{FG}">{esc(title)}</text>'
            )
            y += HDR_H + HDR_PAD_BELOW + LINE_H
            for ln in lines:
                out.append(
                    f'<text x="{x + 4}" y="{y}" font-family="{FONT}" '
                    f'font-size="7.6" fill="{FG}">{esc(ln)}</text>'
                )
                y += LINE_H
            y += SECTION_GAP
    out.append("</svg>")
    return "\n".join(out)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    root = os.path.dirname(here)
    svg_path = os.path.join(here, "midip-cheatsheet.svg")
    pdf_path = os.path.join(root, "midip-cheatsheet.pdf")

    with open(svg_path, "w", encoding="utf-8") as f:
        f.write(build_svg())
    print(f"wrote {os.path.relpath(svg_path, root)}")

    rsvg = shutil.which("rsvg-convert")
    if not rsvg:
        print(
            "rsvg-convert not found — SVG written, PDF NOT rebuilt.\n"
            "  install: brew install librsvg  |  apt install librsvg2-bin",
            file=sys.stderr,
        )
        sys.exit(1)
    subprocess.run([rsvg, "-f", "pdf", "-o", pdf_path, svg_path], check=True)
    print(f"wrote {os.path.relpath(pdf_path, root)}")


if __name__ == "__main__":
    main()
