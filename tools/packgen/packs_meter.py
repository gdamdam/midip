"""Phase 9: multi-bar (32/48/64), alternate-meter (3/4, 6/8, 5/4) and polymetric
(12/15/20-step) content. All lengths play via the engine's per-lane length wrap;
meter is declared in free-form metadata (meter/steps_per_bar/bars) + a tag.
T-8 drum voices only."""
from engine import row as R, family
from helpers import drums, bass, synth, prov

REF = ["Magenta Groove MIDI Dataset", "documented genre groove references",
       "standard meter/subdivision theory (3/4, 6/8, 5/4)"]


def dbl(base):
    """Duplicate a dict of 16-char grid rows into 32-char (two-bar) rows."""
    return {v: g + g for v, g in base.items()}


def pack_meter():
    P = prov("multi-bar", REF)

    # ================= 1. Two-bar (32-step) T-8 drum patterns =================
    # Each: a 16-step base groove doubled to 2 bars, with a bar-2 end fill
    # (tom move + ratcheted snare on the last beat, steps 28-31).
    def twobar(genre, name, base, *, desc, bpm, energy, timing=None):
        g = dbl(base)
        # bar-2 end fill on the last beat (idx 28-31): a tom accent + a ratcheted
        # snare roll into the downbeat of the loop.
        mt = list(g.get("MT", "." * 32))
        mt[26] = "x"
        g["MT"] = "".join(mt)
        sd = list(g.get("SD", "." * 32))
        for k in (28, 29, 30, 31):
            sd[k] = "x"
        g["SD"] = "".join(sd)
        return drums(genre, name, "variation_b", g,
                     ratch={"SD": {29: 2, 30: 3, 31: 4}}, length=32,
                     desc=desc, bpm=bpm, feel="", energy=energy, density="dense",
                     tags=[genre, "two-bar"], prov=P, meter="4/4", spb=16, bars=2,
                     timing=timing)

    two = [
        ("jungle-breaks", "Amen Two-Bar", {"BD": R(16, {0: 'X', 10: 'x'}), "SD": R(16, {4: 'X', 12: 'X', 7: 'o'}), "CH": R(16, {i: 'x' for i in range(16)})}, "Chopped two-bar amen break with a ratcheted snare fill into the loop.", (160, 174), "high"),
        ("drum-and-bass", "Roller Two-Bar", {"BD": R(16, {0: 'X', 10: 'x'}), "SD": R(16, {4: 'X', 12: 'X'}), "CH": R(16, {i: 'x' for i in range(16)})}, "Rolling two-bar DnB groove with a bar-2 turnaround.", (170, 176), "high"),
        ("breakbeat", "Funky Two-Bar", {"BD": R(16, {0: 'X', 6: 'x', 10: 'x'}), "SD": R(16, {4: 'X', 12: 'X'}), "CH": R(16, {0: 'x', 2: 'x', 4: 'x', 6: 'x', 8: 'x', 10: 'x', 12: 'x', 14: 'x'})}, "Funky two-bar break with tom turnaround.", (110, 130), "mid"),
        ("boom-bap", "Turnaround Two-Bar", {"BD": R(16, {0: 'X', 6: 'x', 10: 'x'}), "SD": R(16, {4: 'X', 12: 'X'}), "CH": R(16, {0: 'x', 2: 'x', 4: 'x', 6: 'x', 8: 'x', 10: 'x', 12: 'x', 14: 'x'})}, "Two-bar boom-bap with a bar-2 snare turnaround.", (82, 96), "mid", "mpc-swing"),
        ("techno", "Warehouse Two-Bar", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "OH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'}), "CH": R(16, {i: 'x' for i in range(16)})}, "Two-bar warehouse build with a bar-2 fill.", (128, 138), "high"),
        ("garage", "2-Step Two-Bar", {"BD": R(16, {0: 'X', 6: 'x'}), "SD": R(16, {4: 'X', 12: 'X'}), "CH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'})}, "Two-bar UK 2-step with a bar-2 skip variation.", (130, 138), "mid"),
        ("footwork", "Juke Two-Bar", {"BD": R(16, {0: 'X', 3: 'x', 6: 'x', 10: 'x', 13: 'x'}), "SD": R(16, {8: 'X'}), "CH": R(16, {i: 'x' for i in range(16)})}, "Two-bar footwork with a triplet-ratchet fill.", (158, 162), "high"),
        ("house", "Jack Two-Bar", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "SD": R(16, {4: 'X', 12: 'X'}), "OH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'}), "CH": R(16, {i: 'x' for i in range(16)}), "RS": R(16, {3: 'x', 11: 'x'})}, "Two-bar jackin' house with sixteenth hats, a rim tick and a bar-2 build.", (120, 126), "mid"),
        ("funk", "Pocket Two-Bar", {"BD": R(16, {0: 'X', 6: 'x', 10: 'x'}), "SD": R(16, {4: 'X', 12: 'X', 7: 'o', 11: 'o'}), "CH": R(16, {i: ('X' if i in (0, 4, 8, 12) else 'x') for i in range(16)})}, "Two-bar funk pocket with ghost snares and a bar-2 fill.", (96, 112), "high", "light-swing"),
        ("disco", "Floor Two-Bar", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "SD": R(16, {4: 'X', 12: 'X'}), "OH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'}), "CH": R(16, {0: 'x', 2: 'x', 4: 'x', 6: 'x', 8: 'x', 10: 'x', 12: 'x', 14: 'x'})}, "Two-bar disco four-on-floor with a bar-2 crescendo.", (115, 125), "high"),
        ("dancehall", "Dembow Two-Bar", {"BD": R(16, {0: 'X', 8: 'x'}), "SD": R(16, {3: 'x', 6: 'X', 11: 'x', 14: 'X'}), "CH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'})}, "Two-bar dembow riddim with a bar-2 roll.", (88, 105), "mid"),
        ("reggaeton", "Perreo Two-Bar", {"BD": R(16, {0: 'X', 4: 'x', 8: 'X', 12: 'x'}), "SD": R(16, {3: 'x', 6: 'X', 11: 'x', 14: 'X'}), "CH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'})}, "Two-bar reggaeton with a bar-2 perreo roll.", (88, 100), "mid"),
        ("amapiano", "Log Bed Two-Bar", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "CH": R(16, {i: 'x' for i in range(16)}), "OH": R(16, {6: 'x', 14: 'x'})}, "Two-bar amapiano bed for the log drum, bar-2 percussion shift.", (108, 118), "mid"),
        ("afro-house", "Perc Two-Bar", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "OH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'}), "CH": R(16, {i: 'x' for i in range(16)}), "CB": R(16, {0: 'x', 3: 'x', 6: 'x', 10: 'x', 12: 'x'})}, "Two-bar afro-house with a bar-2 tom layer.", (118, 125), "mid"),
        ("tech-house", "Roller Two-Bar", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "OH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'}), "CH": R(16, {i: 'x' for i in range(16)}), "RS": R(16, {4: 'x', 12: 'x'})}, "Two-bar tech-house roller with a bar-2 cowbell tool.", (124, 128), "mid"),
        ("trap", "Roll Two-Bar", {"BD": R(16, {0: 'X', 6: 'x', 10: 'x'}), "SD": R(16, {8: 'X'}), "CH": R(16, {i: ('X' if i in (0, 4, 8, 12) else 'x') for i in range(16)})}, "Two-bar half-time trap with an escalating bar-2 hat roll.", (130, 150), "high"),
    ]
    for spec in two:
        genre, name, base, desc, bpm, energy = spec[0], spec[1], spec[2], spec[3], spec[4], spec[5]
        timing = spec[6] if len(spec) > 6 else None
        twobar(genre, name, base, desc=desc, bpm=bpm, energy=energy, timing=timing)

    # ================= 2. Multi-bar synth / bass phrases =================
    def phr(role, genre, name, notes, length, bars, desc, bpm, harmonic):
        fn = synth if role == "synth" else bass
        fn(genre, name, "variation_b", notes, length=length, desc=desc, bpm=bpm,
           feel="", energy="mid", density="core", tags=[genre, f"{bars}-bar"],
           prov=P, harmonic=harmonic, meter="4/4", spb=16, bars=bars)

    # 32-step (2-bar)
    phr("synth", "melodic-techno", "Two-Bar Minor Arp", [(i, [0, 7, 12, 15, 19][i % 5], 0.4, 'n') for i in range(0, 32, 2)], 32, 2, "Two-bar rolling minor arpeggio.", (120, 126), "minor")
    phr("bass", "tech-house", "Two-Bar Roller Bass", [(s, 0 if (s // 2) % 2 == 0 else -2, 0.4, 'n') for s in range(2, 32, 4)], 32, 2, "Two-bar rolling offbeat bass.", (124, 128), "minor")
    phr("synth", "trap", "Two-Bar Phrygian", [(0, 0, 1.0, 'a'), (6, 1, 0.5, 'n'), (10, 3, 1.0, 'n'), (16, -2, 1.0, 'n'), (22, 1, 0.5, 'n'), (26, 0, 2.0, 'n')], 32, 2, "Two-bar dark Phrygian motif.", (130, 150), "phrygian")
    phr("bass", "disco", "Two-Bar Octave Walk", [(s, 0 if s % 4 == 0 else 12, 0.5, 'n') for s in range(0, 32, 2)], 32, 2, "Two-bar disco octave bass with a walk-up.", (115, 125), "major")
    # Multi-bar chord phrases (house Two-Bar Stab Vamp; melodic-techno Three-Bar
    # Progression & Four-Bar Chords; amapiano Three-Bar Jazzy Vamp; deep-house
    # Four-Bar Pads; disco Four-Bar Strings) moved to role "chords" (packs_chords.py).
    # 48-step (3-bar)
    phr("bass", "afro-house", "Three-Bar Roll", [(s, [0, 3, 7, 0][(s // 4) % 4], 0.5, 'n') for s in range(0, 48, 2)], 48, 3, "Three-bar rolling pentatonic afro bass.", (118, 125), "minor-pentatonic")
    # 64-step (4-bar)
    phr("bass", "trap", "Four-Bar 808 Line", [(0, 0, 3.0, 'a'), (10, 0, 1.0, 'n'), (16, 3, 2.0, 'n', 'slide'), (32, -2, 3.0, 'n', 'slide'), (48, 0, 3.0, 'n')], 64, 4, "Four-bar melodic 808 sub line with glides.", (130, 150), "minor")

    # ================= 3. 3/4 & 6/8 families (steps_per_bar 12) =================
    g = "waltz"
    c = drums(g, "Waltz Core", "core",
              {"BD": R(12, {0: 'X'}), "SD": R(12, {4: 'X', 8: 'X'}), "CH": R(12, {i: 'x' for i in range(12)})},
              length=12, desc="3/4 waltz: kick on beat 1, snare on beats 2 & 3, steady eighth hats.",
              bpm=(90, 140), feel="", energy="mid", density="core", tags=["waltz"], prov=P,
              meter="3/4", spb=12, bars=1)
    d = drums(g, "Jazz Waltz", "variation_a",
              {"BD": R(24, {0: 'X', 12: 'X', 16: 'x'}), "RS": R(24, {4: 'x', 8: 'x', 16: 'x', 20: 'x'}), "RC": R(24, {i: 'x' for i in range(0, 24, 2)})},
              length=24, desc="Two-bar 3/4 jazz waltz with a ride pattern and a bar-2 turnaround.",
              bpm=(100, 160), feel="", energy="mid", density="core", tags=["waltz", "jazz"], prov=P,
              meter="3/4", spb=12, bars=2)
    family("meter-waltz-drums", "Waltz", "drums", g, [("core", c), ("sparse", d)])

    g = "six-eight"
    c = drums(g, "6/8 Shuffle", "core",
              {"BD": R(12, {0: 'X', 6: 'x'}), "SD": R(12, {6: 'X'}), "CH": R(12, {0: 'x', 2: 'x', 4: 'x', 6: 'x', 8: 'x', 10: 'x'})},
              length=12, desc="6/8 groove felt in two dotted-quarter groups: kick on 1 & 4, snare on 4.",
              bpm=(60, 100), feel="", energy="mid", density="core", tags=["six-eight"], prov=P,
              meter="6/8", spb=12, bars=1)
    v = drums(g, "6/8 Open", "variation_a",
              {"BD": R(12, {0: 'X', 6: 'x'}), "SD": R(12, {6: 'X'}), "OH": R(12, {0: 'x', 6: 'x'}), "CH": R(12, {2: 'x', 4: 'x', 8: 'x', 10: 'x'})},
              length=12, desc="6/8 with open-hat accents on each group head.",
              bpm=(60, 100), feel="", energy="mid", density="sparse", tags=["six-eight"], prov=P,
              meter="6/8", spb=12, bars=1)
    family("meter-six-eight-drums", "6/8 Shuffle", "drums", g, [("core", c), ("sparse", v)])

    # ================= 4. 5/4 drums (20-step) =================
    g = "five-four"
    c = drums(g, "[5/4] Straight", "core",
              {"BD": R(20, {0: 'X', 8: 'x', 12: 'X'}), "SD": R(20, {4: 'X', 16: 'X'}), "CH": R(20, {i: 'x' for i in range(0, 20, 2)})},
              length=20, desc="Straight 5/4: five beats, kick anchors, backbeat across the bar.",
              bpm=(90, 140), feel="", energy="mid", density="core", tags=["five-four"], prov=P,
              meter="5/4", spb=20, bars=1)
    d = drums(g, "[5/4] Displaced", "variation_a",
              {"BD": R(20, {0: 'X', 6: 'x', 14: 'x'}), "SD": R(20, {16: 'X', 18: 'x'}), "CH": R(20, {i: 'x' for i in range(20)})},
              length=20, desc="5/4 with a displaced snare accent on beat 5 and a tom lead-in.",
              bpm=(90, 140), feel="", energy="high", density="dense", tags=["five-four"], prov=P,
              meter="5/4", spb=20, bars=1)
    j = drums(g, "[5/4] Jazz Ride", "variation_b",
              {"BD": R(20, {0: 'x', 8: 'x'}), "RS": R(20, {4: 'x', 12: 'x'}), "RC": R(20, {i: 'x' for i in range(0, 20, 2)}), "CB": R(20, {16: 'x'})},
              length=20, desc="5/4 jazz ride comping with a cowbell accent on beat 5.",
              bpm=(120, 200), feel="", energy="mid", density="core", tags=["five-four", "jazz"], prov=P,
              meter="5/4", spb=20, bars=1)
    family("meter-five-four-drums", "5/4", "drums", g, [("core", c), ("sparse", d), ("dense", j)])

    # ================= 5. Polymetric loops (12/15/20 vs 16) =================
    g = "polymeter"
    poly_prov = prov("multi-bar", REF + ["intentional polymeter (loop length ≠ 4/4 multiple)"])

    def poly_drums(name, length, grids, desc, ratio):
        return drums(g, name, "core", grids, length=length,
                     desc=desc, bpm=(120, 140), feel="", energy="mid", density="core",
                     tags=["polymeter", ratio], prov=poly_prov, meter="poly", spb=length, bars=1)

    a = poly_drums("Poly 12 (3:4)", 12, {"CB": R(12, {0: 'X', 4: 'x', 8: 'x'}), "CH": R(12, {i: 'x' for i in range(12)}), "RS": R(12, {2: 'x', 6: 'x', 10: 'x'})}, "12-step cowbell/rim ostinato that phases 3:4 against a 16-step lane (48-step cycle).", "3:4")
    b = poly_drums("Poly 15 (15:16)", 15, {"RS": R(15, {0: 'X', 5: 'x', 10: 'x'}), "CH": R(15, {i: 'x' for i in range(15)}), "MT": R(15, {7: 'x', 12: 'x'})}, "15-step motif with a slow 15:16 drift against 4/4 (240-step cycle).", "15:16")
    c = poly_drums("Poly 20 (5:4)", 20, {"BD": R(20, {0: 'X', 8: 'x', 12: 'x'}), "CH": R(20, {i: 'x' for i in range(0, 20, 2)}), "CB": R(20, {4: 'x', 16: 'x'})}, "20-step kick/bell ostinato phasing 5:4 against a 16-step lane (80-step cycle).", "5:4")
    family("meter-polymeter-drums", "Polymeter", "drums", g, [("core", a), ("sparse", b), ("dense", c)])

    bass(g, "Poly 12 Bass", "core",
         [(0, 0, 0.5, 'a'), (3, 3, 0.5, 'n'), (6, 7, 0.5, 'n'), (9, 3, 0.5, 'n')],
         length=12, desc="12-step bass arpeggio phasing 3:4 under a 16-step drum lane.",
         bpm=(120, 140), feel="", energy="mid", density="core", tags=["polymeter", "3:4"],
         prov=poly_prov, harmonic="minor", meter="poly", spb=12, bars=1)
    family("meter-polymeter-bass", "Polymeter Bass", "bass", g,
           [("core", "Poly 12 Bass")])
