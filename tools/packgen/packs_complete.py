"""Phase 10: coverage-driven completion. Adds the missing Fill / Breakdown /
variation members to the highest-value flagship drum families (evidence: 44/71
families lacked a Fill, 43 lacked a Breakdown). Prefers COMPLETING families over
new names; breakdowns also fill the sparse / low-energy gap. T-8 voices only."""
from engine import row as R, extend_family
from helpers import drums

P_REF = ["Magenta Groove MIDI Dataset", "documented genre fill/breakdown conventions"]


def pack_complete():
    from helpers import prov
    P = prov("coverage-completion", P_REF)

    def fill(genre, name, grids, *, bpm, ratch=None, timing=None):
        return drums(genre, name, "fill", grids, ratch=ratch, length=16,
                     desc=f"Bar-end fill for the {genre} groove — tom move and a ratcheted "
                          f"snare roll into a crash on the one.",
                     bpm=bpm, feel="", energy="high", density="dense",
                     tags=[genre, "fill"], prov=P, timing=timing)

    def brk(genre, name, grids, *, bpm, desc, timing=None):
        return drums(genre, name, "breakdown", grids, length=16, desc=desc,
                     bpm=bpm, feel="", energy="low", density="sparse",
                     tags=[genre, "breakdown"], prov=P, timing=timing)

    # Standard bar-end fill skeleton (reused, genre BPM varies).
    FILLG = {"BD": R(16, {0: 'X'}), "SD": R(16, {12: 'x', 13: 'x', 14: 'x', 15: 'X'}),
             "MT": R(16, {8: 'x', 9: 'x'}), "HT": R(16, {10: 'x', 11: 'x'}), "CC": R(16, {0: 'X'})}
    FILLR = {"SD": {13: 2, 14: 3, 15: 4}}

    # --- dancehall (missing fill + breakdown) ---
    f = fill("dancehall", "Dembow Fill", FILLG, bpm=(88, 105), ratch=FILLR)
    b = brk("dancehall", "Dembow Drop", {"BD": R(16, {0: 'X'}), "CH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'})},
            bpm=(88, 105), desc="Dancehall breakdown: kick on 1 and the offbeat hat tick only — space for the vocal.")
    extend_family("dancehall-drums-dembow", [("fill", f), ("breakdown", b)])

    # --- reggaeton (missing breakdown) ---
    b = brk("reggaeton", "Perreo Drop", {"BD": R(16, {0: 'X', 8: 'X'}), "CH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'})},
            bpm=(88, 100), desc="Reggaeton breakdown: kick on 1 & 3 and offbeat hats, snare dropped.")
    extend_family("reggaeton-drums-dembow", [("breakdown", b)])

    # --- footwork (missing fill + breakdown) ---
    f = fill("footwork", "Footwork Fill", {"BD": R(16, {0: 'X', 6: 'x'}), "SD": R(16, {8: 'x', 12: 'x', 14: 'x'}),
             "CH": R(16, {i: 'x' for i in range(16)}), "CC": R(16, {0: 'X'})},
             bpm=(158, 162), ratch={"SD": {8: 3, 12: 3, 14: 3}})
    b = brk("footwork", "Sparse Drop", {"BD": R(16, {0: 'X', 6: 'x', 12: 'x'}), "CH": R(16, {0: 'x', 4: 'x', 8: 'x', 12: 'x'})},
            bpm=(158, 162), desc="Footwork breakdown: syncopated kick and quarter hats, claps dropped.")
    extend_family("footwork-drums-skitter", [("fill", f), ("breakdown", b)])

    # --- tech-house (missing breakdown) ---
    b = brk("tech-house", "Filter Drop", {"OH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'}), "CH": R(16, {i: 'x' for i in range(16)})},
            bpm=(124, 128), desc="Tech-house breakdown: offbeat open hats and rolling closed hats, kick dropped for the filter build.")
    extend_family("tech-house-drums-roller", [("breakdown", b)])

    # --- hard-techno (missing variation_a + breakdown) ---
    v = drums("hard-techno", "Rolling Offbeat", "variation_a", {"BD": R(16, {0: '^', 4: '^', 8: '^', 12: '^'}), "OH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'}), "CH": R(16, {i: 'x' for i in range(16)}), "RS": R(16, {7: 'x', 15: 'x'})},
              length=16, desc="Hard-techno variation: four-on-floor with an offbeat rim accent.",
              bpm=(140, 155), feel="", energy="high", density="core", tags=["hard-techno"], prov=P)
    b = brk("hard-techno", "Hard Drop", {"OH": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'}), "CH": R(16, {i: 'x' for i in range(16)}), "MT": R(16, {4: 'x', 12: 'x'}), "HT": R(16, {7: 'x', 15: 'x'})},
            bpm=(140, 155), desc="Hard-techno breakdown: relentless hats with a tribal tom pulse over a dropped kick.")
    extend_family("hard-techno-drums-groove", [("sparse", v), ("breakdown", b)])

    # --- afro-house (missing breakdown) ---
    b = brk("afro-house", "Afro Drop", {"CB": R(16, {0: 'x', 3: 'x', 6: 'x', 10: 'x', 12: 'x'}), "CH": R(16, {i: 'x' for i in range(16)}), "RS": R(16, {2: 'x', 9: 'x', 13: 'x'})},
            bpm=(118, 125), desc="Afro-house breakdown: the bell timeline and percussion carry it while the kick drops out.")
    extend_family("afro-house-drums-groove", [("breakdown", b)])

    # --- amapiano (missing fill) ---
    f = fill("amapiano", "Piano Fill", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X'}), "SD": R(16, {12: 'x', 14: 'x', 15: 'x'}),
             "RS": R(16, {9: 'x', 11: 'x'}), "CH": R(16, {i: 'x' for i in range(16)})},
             bpm=(108, 118), ratch={"SD": {14: 3, 15: 4}})
    extend_family("amapiano-drums-core", [("fill", f)])

    # --- melodic-techno (missing variation_a + fill) ---
    v = drums("melodic-techno", "Sparse Pulse", "variation_a", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "RC": R(16, {2: 'x', 6: 'x', 10: 'x', 14: 'x'})},
              length=16, desc="Melodic-techno variation: bare four-on-floor with a ride shimmer.",
              bpm=(120, 126), feel="", energy="low", density="sparse", tags=["melodic-techno"], prov=P)
    f = fill("melodic-techno", "Melodic Fill", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X'}), "MT": R(16, {9: 'x', 11: 'x'}), "HT": R(16, {13: 'x', 15: 'x'}), "CC": R(16, {0: 'X'})},
             bpm=(120, 126), ratch={"HT": {15: 3}})
    extend_family("melodic-techno-drums-pulse", [("sparse", v), ("fill", f)])

    # --- reggae steppers (missing fill + breakdown) ---
    f = fill("reggae", "Steppers Fill", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "RS": R(16, {13: 'x', 14: 'x', 15: 'x'}), "MT": R(16, {9: 'x'}), "HT": R(16, {11: 'x'})},
             bpm=(70, 92), ratch={"RS": {14: 2, 15: 3}})
    b = brk("reggae", "Steppers Dub", {"BD": R(16, {0: 'X', 4: 'X', 8: 'X', 12: 'X'}), "RS": R(16, {8: 'X'})},
            bpm=(70, 92), desc="Dub breakdown of the steppers: four-on-floor kick and the cross-stick on 3, everything else stripped for echo.")
    extend_family("reggae-drums-steppers", [("fill", f), ("breakdown", b)])
