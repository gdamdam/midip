"""Curated CHORDS factory pack (role: "chords").

Hardware-neutral polyphonic chord patterns for the J-6-style chords lane.
Every chord step has <=4 simultaneous voices (engine MAX_CHORD_VOICES=4).
Voicings sit in a musical mid register (offsets ~ -9..24 from the lane root,
J-6 root_note = 48 = C3); the low register is left for a bass line. Harmony is
original/generic (no copyrighted songs). Timing is straight (no baked swing).

This pack also HOSTS the six seed chord patterns and the other genuinely-chordal
synth patterns that were reclassified from role "synth" to role "chords"
(same genre + display name), revoiced to <=4 voices. The Rust loader bridges
old synth-role PatternRefs to these by genre+name (library.rs::find_aliased).
"""
from engine import emit, family, mel_steps, cc_slots

COMPAT = ["j-6", "s1", "minilogue-xd", "microfreak", "generic-poly-synth"]
PROV = {"source": "midip factory", "author": "midip", "pack": "chords-v1",
        "license": "CC0-1.0",
        "references": ["https://www.roland.com/GLOBAL/products/j-6/"]}


def chords(genre, name, function, notes, *, desc, bpm, energy, density, tags,
           harmonic, length=16, cc=None, bars=1):
    """Emit one chords-role melodic pattern. feel/timing fixed straight."""
    steps = mel_steps(length, notes)
    meta = dict(desc=desc, bpm_min=bpm[0], bpm_max=bpm[1], feel="straight",
                timing="straight", energy=energy, density=density,
                tags=list(tags) + [harmonic, "chords"], harmonic=harmonic,
                chord_poly="chord", subgenre=genre, compatible_devices=COMPAT)
    if length > 16:
        meta["meter"] = "4/4"; meta["steps_per_bar"] = 16; meta["bars"] = bars
        meta["tags"] = list(meta["tags"]) + [f"{bars}-bar"]
    emit("chords", genre, name, function, steps, "melodic", length, meta, PROV,
         cc=cc_slots(length, cc) if cc else None)
    return name


def pack_chords():
    # ==================================================================
    # A. MOVED seed + reclassified chord patterns (revoiced to <=4 voices)
    # ==================================================================
    # --- afro-house: Deep Pad Vamp (min9) — seed, revoiced 5->4 ---
    chords("afro-house", "Deep Pad Vamp", "core",
        [(0, [0, 3, 10, 14], 4.0, 'n'), (8, [-2, 3, 7, 10], 4.0, 'n')],
        desc="Two-chord minor-ninth pad vamp (i to bVII) with a slow filter opening.",
        bpm=(118, 125), energy="mid", density="core",
        tags=["afro-house", "pad", "marimba"], harmonic="min9",
        cc={0: [(74, 35)], 8: [(74, 95)]})

    # --- amapiano: Jazzy Keys / Two-Chord Vamp / Three-Bar Jazzy Vamp ---
    chords("amapiano", "Jazzy Keys", "core",
        [(0, [0, 3, 10, 14], 2.0, 'n'), (6, [-2, 2, 9, 12], 2.0, 'n'),
         (10, [3, 7, 14, 17], 1.5, 'a'), (14, [-2, 2, 5, 9], 1.0, 'n')],
        desc="Warm jazzy minor-ninth piano chords moving through a four-chord progression.",
        bpm=(108, 118), energy="mid", density="core",
        tags=["amapiano", "jazz", "keys"], harmonic="min9")
    chords("amapiano", "Two-Chord Vamp", "sparse",
        [(0, [0, 3, 10, 14], 3.5, 'n'), (8, [-4, 0, 7, 10], 3.5, 'n')],
        desc="Relaxed two-chord jazzy vamp (i to bVI) for hooks and intros.",
        bpm=(108, 118), energy="low", density="sparse",
        tags=["amapiano", "jazz", "keys"], harmonic="min9")
    chords("amapiano", "Three-Bar Jazzy Vamp", "dense",
        [(0, [0, 3, 10, 14], 3.0, 'n'), (16, [-2, 2, 9, 12], 3.0, 'n'),
         (32, [3, 7, 10, 14], 3.0, 'a')],
        desc="Three-bar jazzy amapiano chord vamp, one chord per bar.",
        bpm=(108, 118), energy="mid", density="dense",
        tags=["amapiano", "jazz", "keys"], harmonic="min9", length=48, bars=3)

    # --- funk: Clav Ninth Stabs (dom9) — seed, revoiced 5->4 ---
    chords("funk", "Clav Ninth Stabs", "core",
        [(2, [0, 4, 10, 14], 0.3, 'n'), (6, [0, 4, 10, 14], 0.3, 'a'),
         (11, [0, 4, 10, 14], 0.25, 'n'), (14, [0, 4, 10, 14], 0.3, 'n')],
        desc="Scratchy off-beat dominant-ninth clav stabs over a one-chord vamp.",
        bpm=(96, 112), energy="high", density="core",
        tags=["funk", "clav", "stabs"], harmonic="dom9")

    # --- tech-house: Minimal Min9 Stab (min9) — seed, revoiced 5->4 ---
    chords("tech-house", "Minimal Min9 Stab", "core",
        [(2, [0, 3, 10, 14], 0.3, 'n'), (10, [0, 3, 10, 14], 0.3, 'a')],
        desc="Two minimal minor-ninth organ stabs — the whole harmonic hook.",
        bpm=(124, 128), energy="mid", density="core",
        tags=["tech-house", "stab"], harmonic="min9")

    # --- boom-bap: Rhodes Min7 Stabs / Jazz ii-V-i (already <=4) ---
    chords("boom-bap", "Rhodes Min7 Stabs", "core",
        [(2, [0, 3, 7, 10], 0.4, 'n'), (6, [0, 3, 7, 10], 0.4, 'n'),
         (10, [-2, 2, 5, 9], 0.4, 'n'), (14, [0, 3, 7, 10], 0.4, 'a')],
        desc="Off-beat Rhodes minor-seventh chord stabs; a dusty two-chord vamp.",
        bpm=(82, 98), energy="mid", density="core",
        tags=["hip-hop", "boom-bap", "keys"], harmonic="min7")
    chords("boom-bap", "Jazz ii-V-i", "dense",
        [(0, [5, 9, 12, 15], 1.5, 'n'), (4, [10, 14, 17, 20], 1.5, 'n'),
         (8, [0, 3, 7, 10], 2.5, 'a')],
        desc="A ii-V-i in minor: held Dm7, G7, then a resting Am7 chord.",
        bpm=(82, 98), energy="mid", density="dense",
        tags=["hip-hop", "boom-bap", "keys"], harmonic="min7")

    # --- trap: Minor Bell Stabs (minor triad) ---
    chords("trap", "Minor Bell Stabs", "core",
        [(0, [0, 3, 7], 0.5, 'a'), (6, [0, 3, 7], 0.5, 'n'),
         (8, [-1, 3, 7], 0.5, 'n'), (12, [0, 3, 7], 0.5, 'n')],
        desc="Dark minor-triad bell stabs — a sparse, ominous trap motif.",
        bpm=(130, 150), energy="mid", density="core",
        tags=["trap", "dark", "keys"], harmonic="minor")

    # --- melodic-techno: Melodic Breakdown / Three-Bar Prog / Four-Bar Chords ---
    chords("melodic-techno", "Melodic Breakdown", "breakdown",
        [(0, [0, 3, 7, 10], 6.0, 'a'), (8, [-2, 3, 7, 10], 6.0, 'n')],
        desc="Two sustained minor-ninth pad chords for the breakdown, with a slow filter sweep.",
        bpm=(120, 126), energy="low", density="sparse",
        tags=["melodic-techno", "deep"], harmonic="min9",
        cc={0: [(74, 20)], 8: [(74, 70)]})
    chords("melodic-techno", "Three-Bar Progression", "sparse",
        [(0, [0, 3, 7], 4.0, 'n'), (16, [8, 12, 15], 4.0, 'n'),
         (32, [3, 7, 10], 4.0, 'a')],
        desc="Three-chord melodic-techno progression, one per bar (i-VI-III).",
        bpm=(120, 126), energy="mid", density="sparse",
        tags=["melodic-techno", "deep"], harmonic="minor", length=48, bars=3)
    chords("melodic-techno", "Four-Bar Chords", "core",
        [(0, [0, 3, 7], 8.0, 'n'), (16, [8, 12, 15], 8.0, 'n'),
         (32, [3, 7, 10], 8.0, 'n'), (48, [10, 14, 17], 8.0, 'a')],
        desc="Four-bar melodic-techno chord progression (i-VI-III-VII).",
        bpm=(120, 126), energy="mid", density="core",
        tags=["melodic-techno", "deep"], harmonic="minor", length=64, bars=4)

    # --- disco: String Stabs / ii-V-I Strings / Four-Bar Strings (maj7) ---
    chords("disco", "String Stabs", "core",
        [(0, [0, 4, 7, 11], 1.0, 'a'), (4, [0, 4, 7, 11], 0.5, 'n'),
         (8, [-3, 0, 4, 7], 1.0, 'n'), (12, [-3, 0, 4, 7], 0.5, 'n')],
        desc="Lush major-seventh string stabs over a two-chord disco vamp.",
        bpm=(115, 125), energy="mid", density="core",
        tags=["disco", "strings"], harmonic="maj7")
    chords("disco", "ii-V-I Strings", "dense",
        [(0, [2, 5, 9, 12], 1.5, 'n'), (4, [7, 11, 14, 17], 1.5, 'n'),
         (8, [0, 4, 7, 11], 2.5, 'a')],
        desc="Classic ii-V-I string progression with a filter sweep on the resolution.",
        bpm=(115, 125), energy="mid", density="dense",
        tags=["disco", "strings"], harmonic="maj7", cc={8: [(74, 110)]})
    chords("disco", "Four-Bar Strings", "sparse",
        [(0, [0, 4, 7, 11], 3.0, 'n'), (16, [2, 5, 9, 12], 3.0, 'n'),
         (32, [7, 11, 14], 3.0, 'n'), (48, [0, 4, 7, 11], 4.0, 'a')],
        desc="Four-bar disco string ii-V-I turnaround.",
        bpm=(115, 125), energy="mid", density="sparse",
        tags=["disco", "strings"], harmonic="maj7", length=64, bars=4)

    # --- reggae: Offbeat Skank / Bubble Organ (minor triad, upbeats) ---
    chords("reggae", "Offbeat Skank", "core",
        [(2, [0, 3, 7], 0.4, 'n'), (6, [0, 3, 7], 0.4, 'n'),
         (10, [0, 3, 7], 0.4, 'n'), (14, [0, 3, 7], 0.4, 'n')],
        desc="The reggae identity: short minor-triad organ chops on every off-beat.",
        bpm=(70, 90), energy="mid", density="core",
        tags=["reggae", "skank", "organ", "offbeat"], harmonic="minor")
    chords("reggae", "Bubble Organ", "dense",
        [(2, [0, 3, 7], 0.25, 'n'), (3, [0, 3, 7], 0.25, 's'),
         (6, [0, 3, 7], 0.25, 'n'), (7, [0, 3, 7], 0.25, 's'),
         (10, [0, 3, 7], 0.25, 'n'), (11, [0, 3, 7], 0.25, 's'),
         (14, [0, 3, 7], 0.25, 'n'), (15, [0, 3, 7], 0.25, 's')],
        desc="Double-skank bubble organ: two sixteenth chops per off-beat for the rolling bubble feel.",
        bpm=(70, 90), energy="mid", density="dense",
        tags=["reggae", "skank", "organ", "offbeat"], harmonic="minor")
    chords("reggae", "Steppers Skank", "sparse",
        [(2, [0, 3, 7, 10], 0.35, 'n'), (6, [0, 3, 7, 10], 0.35, 'n'),
         (10, [-2, 2, 5, 9], 0.35, 'n'), (14, [-2, 2, 5, 9], 0.35, 'a')],
        desc="Minor-seventh offbeat skank over a two-chord riddim (i7 to bVII).",
        bpm=(70, 92), energy="mid", density="sparse",
        tags=["reggae", "skank", "organ", "offbeat"], harmonic="min7")

    # --- reggaeton: Dark Minor Stabs (minor triad) ---
    chords("reggaeton", "Dark Minor Stabs", "core",
        [(0, [0, 3, 7], 0.5, 'a'), (4, [-4, 0, 3], 0.5, 'n'),
         (8, [-2, 1, 5], 0.5, 'n'), (12, [3, 7, 10], 0.5, 'n')],
        desc="Dark minor triad stabs following a i-VI-VII-type progression.",
        bpm=(88, 100), energy="mid", density="core",
        tags=["reggaeton", "dark", "keys"], harmonic="minor")
    chords("reggaeton", "Minor Add9 Stab", "sparse",
        [(0, [0, 3, 7, 14], 0.5, 'a'), (4, [-4, 0, 3, 10], 0.5, 'n'),
         (8, [-2, 1, 5, 12], 0.5, 'n'), (12, [3, 7, 10, 17], 0.5, 'n')],
        desc="Minor add-nine stab progression adding an upper ninth colour tone.",
        bpm=(88, 100), energy="mid", density="sparse",
        tags=["reggaeton", "dark", "keys"], harmonic="minor-add9")

    # --- trap variation: Dark Add9 Stabs ---
    chords("trap", "Dark Add9 Stabs", "sparse",
        [(0, [0, 3, 14], 0.5, 'a'), (6, [0, 3, 14], 0.5, 'n'),
         (8, [-1, 3, 14], 0.5, 'n'), (12, [0, 3, 15], 0.5, 'n')],
        desc="Sparse minor add-nine stabs with a fixed upper colour tone for a colder motif.",
        bpm=(130, 150), energy="mid", density="sparse",
        tags=["trap", "dark", "keys"], harmonic="minor-add9")

    # --- house: Two-Bar Stab Vamp (min7) ---
    chords("house", "Two-Bar Stab Vamp", "sparse",
        [(2, [0, 3, 7, 10], 0.4, 'n'), (10, [0, 3, 7, 10], 0.4, 'n'),
         (18, [-2, 2, 5, 9], 0.4, 'n'), (26, [-2, 2, 5, 9], 0.4, 'a')],
        desc="Two-bar house chord-stab vamp alternating i7 and bVII.",
        bpm=(120, 126), energy="mid", density="sparse",
        tags=["house", "stab"], harmonic="min7", length=32, bars=2)

    # --- deep-house: Four-Bar Pads (maj7) ---
    chords("deep-house", "Four-Bar Pads", "dense",
        [(0, [0, 4, 7, 11], 8.0, 'n'), (16, [-3, 0, 4, 9], 8.0, 'n'),
         (32, [-1, 2, 5, 9], 8.0, 'n'), (48, [-3, 0, 4, 7], 8.0, 'a')],
        desc="Four-bar deep-house pad progression (Imaj7-vi-ii-V feel).",
        bpm=(120, 125), energy="mid", density="dense",
        tags=["deep-house", "pad"], harmonic="maj7", length=64, bars=4)

    # ==================================================================
    # B. NEW curated chord patterns (harmonic-family coverage)
    # ==================================================================
    # --- dub: minor-7 dub stab ---
    chords("dub", "Minor7 Dub Stab", "core",
        [(2, [0, 3, 7, 10], 0.5, 'n'), (6, [0, 3, 7, 10], 0.5, 's', ('prob', 0.85)),
         (10, [-2, 2, 5, 9], 0.5, 'n'), (14, [-2, 2, 5, 9], 0.5, 's', ('prob', 0.7))],
        desc="Off-beat minor-seventh dub stabs (i7 to bVII) with probabilistic echo repeats.",
        bpm=(70, 90), energy="mid", density="core",
        tags=["dub", "stab", "echo"], harmonic="min7")
    chords("dub", "Dub Chord Echo", "sparse",
        [(2, [0, 3, 7, 10], 0.5, 'a'), (5, [0, 3, 7, 10], 0.4, 's', ('prob', 0.5)),
         (18, [-2, 2, 5, 9], 0.5, 'n'), (21, [-2, 2, 5, 9], 0.4, 's', ('prob', 0.4))],
        desc="Two-bar sparse dub stabs with delayed probabilistic echo repeats.",
        bpm=(70, 90), energy="low", density="sparse",
        tags=["dub", "stab", "echo"], harmonic="min7", length=32, bars=2)

    # --- deep-house: minor-9 pulse ---
    chords("deep-house", "Deep Min9 Pulse", "core",
        [(2, [0, 3, 10, 14], 0.5, 'n'), (6, [0, 3, 10, 14], 0.5, 's'),
         (10, [0, 3, 10, 14], 0.5, 'n'), (14, [-2, 2, 9, 12], 0.5, 'a')],
        desc="Pulsing off-beat minor-ninth stabs alternating i9 and bVII.",
        bpm=(120, 125), energy="mid", density="core",
        tags=["deep-house", "pulse", "stab"], harmonic="min9")
    chords("deep-house", "Min9 Pulse Roll", "sparse",
        [(2, [0, 3, 10, 14], 0.5, 'n'), (6, [0, 3, 10, 14], 0.5, 's'),
         (10, [-2, 2, 9, 12], 0.5, 'n'), (18, [3, 7, 14, 17], 0.5, 'n'),
         (22, [3, 7, 14, 17], 0.5, 's'), (26, [-2, 2, 9, 12], 0.5, 'a')],
        desc="Two-bar minor-ninth pulse with a bar-two lift to bIII.",
        bpm=(120, 125), energy="mid", density="sparse",
        tags=["deep-house", "pulse", "stab"], harmonic="min9", length=32, bars=2)

    # --- funk: dominant-9 comp variation ---
    chords("funk", "Dom9 Clav Comp", "sparse",
        [(0, [0, 4, 10, 14], 0.4, 'a'), (3, [0, 4, 10, 14], 0.3, 's'),
         (8, [-2, 2, 8, 12], 0.4, 'n'), (11, [0, 4, 10, 14], 0.3, 'n'),
         (14, [0, 4, 10, 14], 0.3, 's')],
        desc="Syncopated dominant-ninth clav comp trading the tonic 9th with bVII9.",
        bpm=(96, 112), energy="high", density="sparse",
        tags=["funk", "clav", "stabs"], harmonic="dom9")

    # --- lo-fi: major-9 / major-7 pad ---
    chords("lo-fi", "Lo-Fi Maj9 Pad", "core",
        [(0, [0, 4, 11, 14], 8.0, 'n'), (16, [-3, 0, 7, 9], 8.0, 's')],
        desc="Slow major-ninth pad resolving to a relative minor triad (Imaj9 to vi).",
        bpm=(70, 90), energy="low", density="core",
        tags=["lo-fi", "pad", "warm"], harmonic="maj9", length=32, bars=2)
    chords("lo-fi", "Lo-Fi Rhodes Comp", "sparse",
        [(2, [0, 4, 7, 11], 1.0, 'n'), (6, [-3, 0, 4, 9], 1.0, 's'),
         (10, [-1, 2, 5, 9], 1.0, 'n'), (14, [-3, 0, 4, 7], 1.0, 's')],
        desc="Gentle major-seventh Rhodes comp over a Imaj7-vi-ii-V turnaround.",
        bpm=(70, 90), energy="low", density="sparse",
        tags=["lo-fi", "keys", "warm"], harmonic="maj7")

    # --- jazz: Dorian i7-IV7 vamp ---
    chords("jazz", "Dorian Vamp", "core",
        [(0, [0, 3, 7, 10], 4.0, 'n'), (8, [5, 9, 12, 15], 4.0, 'a')],
        desc="Two-chord Dorian vamp: i7 to IV7 (the major-IV Dorian characteristic).",
        bpm=(90, 120), energy="mid", density="core",
        tags=["jazz", "dorian", "vamp"], harmonic="dorian")
    chords("jazz", "Dorian Trade", "sparse",
        [(0, [0, 3, 7, 10], 1.5, 'n'), (6, [0, 3, 7, 10], 0.5, 's'),
         (16, [5, 9, 12, 15], 1.5, 'n'), (22, [5, 9, 12, 15], 0.5, 'a')],
        desc="Two-bar Dorian i7-IV7 trade with syncopated comping accents.",
        bpm=(90, 120), energy="mid", density="sparse",
        tags=["jazz", "dorian", "vamp"], harmonic="dorian", length=32, bars=2)

    # --- synthwave: Aeolian with descending / ascending upper voice ---
    chords("synthwave", "Neon Descent", "core",
        [(0, [0, 3, 10, 19], 8.0, 'n'), (8, [-4, 0, 3, 17], 8.0, 'n'),
         (16, [-5, 3, 7, 15], 8.0, 'n'), (24, [-2, 2, 9, 14], 8.0, 'a')],
        desc="Two-bar Aeolian progression (i-bVI-bIII-bVII) with a descending upper voice (19-17-15-14).",
        bpm=(80, 118), energy="mid", density="core",
        tags=["synthwave", "retro", "descending"], harmonic="aeolian",
        length=32, bars=2)
    chords("synthwave", "Neon Rise", "sparse",
        [(0, [0, 3, 7, 12], 8.0, 'n'), (8, [-2, 2, 5, 14], 8.0, 'n'),
         (16, [-4, 0, 3, 15], 8.0, 'n'), (24, [3, 7, 10, 17], 8.0, 'a')],
        desc="Two-bar minor progression with a stepwise ascending top voice (12-14-15-17).",
        bpm=(80, 118), energy="mid", density="sparse",
        tags=["synthwave", "retro", "ascending"], harmonic="minor",
        length=32, bars=2)

    # --- trance: minor-key EDM lift ---
    chords("trance", "Minor Trance Lift", "core",
        [(0, [0, 3, 7], 8.0, 'n'), (8, [3, 8, 12], 8.0, 'n'),
         (16, [7, 12, 15], 8.0, 'n'), (24, [2, 9, 14], 8.0, 'a')],
        desc="Uplifting minor-key trance progression with a rising chord register across two bars.",
        bpm=(132, 140), energy="high", density="core",
        tags=["trance", "lift", "uplifting"], harmonic="minor", length=32, bars=2)
    chords("trance", "Uplift Chords", "sparse",
        [(0, [0, 3, 7], 3.5, 'n'), (8, [0, 3, 7], 3.5, 's'),
         (16, [-4, 0, 3], 3.5, 'n'), (24, [-5, -1, 3], 3.5, 'a')],
        desc="Two-bar sustained trance chords (i-bVI-bIII) for breakdown-to-drop tension.",
        bpm=(132, 140), energy="mid", density="sparse",
        tags=["trance", "lift", "uplifting"], harmonic="minor", length=32, bars=2)
    chords("trance", "Trance Breakdown", "breakdown",
        [(0, [0, 3, 7, 14], 16.0, 's'), (32, [-4, 0, 3, 10], 16.0, 's')],
        desc="Two long sustained minor add-nine pads for the trance breakdown.",
        bpm=(132, 140), energy="low", density="sparse",
        tags=["trance", "pad", "breakdown"], harmonic="min9", length=64, bars=4)

    # --- ambient: pedal-tone drift ---
    chords("ambient", "Ambient Pedal Drift", "core",
        [(0, [0, 7, 14], 16.0, 's'), (16, [0, 10, 15], 16.0, 's'),
         (32, [0, 9, 14], 16.0, 's'), (48, [0, 7, 12], 16.0, 's')],
        desc="Sustained tonic pedal (root held) under slowly drifting upper voices.",
        bpm=(60, 90), energy="low", density="sparse",
        tags=["ambient", "pedal-tone", "drift", "pad"], harmonic="pedal-tone",
        length=64, bars=4)
    chords("ambient", "Pedal Bloom", "sparse",
        [(0, [0, 5, 9], 16.0, 's', ('prob', 0.9)), (16, [0, 7, 14], 16.0, 's'),
         (32, [0, 5, 12], 16.0, 's', ('prob', 0.8)), (48, [0, 2, 9], 16.0, 's')],
        desc="Pedal-tone drift with open sus/add colours and probabilistic voice swells.",
        bpm=(60, 90), energy="low", density="sparse",
        tags=["ambient", "pedal-tone", "drift", "pad"], harmonic="pedal-tone",
        length=64, bars=4, cc={0: [(74, 20)], 32: [(74, 80)]})

    # --- techno: sparse modal vamp ---
    chords("techno", "Modal Techno Vamp", "core",
        [(0, [0, 7], 2.0, 'n'), (8, [1, 8], 2.0, 'a')],
        desc="Sparse two-note modal vamp: tonic fifth answered by a Phrygian bII fifth.",
        bpm=(128, 140), energy="mid", density="sparse",
        tags=["techno", "modal", "hypnotic"], harmonic="modal")
    chords("techno", "Hypnotic Sus Stab", "sparse",
        [(2, [0, 5, 7], 0.5, 'n'), (6, [0, 5, 7], 0.5, 's'),
         (10, [0, 5, 7], 0.5, 'n'), (14, [-2, 3, 5], 0.5, 'a')],
        desc="Repetitive suspended-fourth stabs with a bar-end voice shift.",
        bpm=(128, 140), energy="mid", density="sparse",
        tags=["techno", "modal", "sus"], harmonic="sus4")
    chords("techno", "Peak Stab Vamp", "peak",
        [(0, [0, 7, 12], 0.5, 'a'), (4, [0, 7, 12], 0.5, 'n'),
         (8, [1, 8, 13], 0.5, 'a'), (12, [0, 7, 12], 0.5, 'n')],
        desc="Driving peak-time fifth stabs with a Phrygian bII push.",
        bpm=(128, 140), energy="high", density="dense",
        tags=["techno", "modal", "peak"], harmonic="modal")

    # --- house: minor-9 pump ---
    chords("house", "House Min9 Pump", "core",
        [(2, [0, 3, 10, 14], 0.4, 'n'), (6, [0, 3, 10, 14], 0.4, 'n'),
         (10, [-2, 2, 9, 12], 0.4, 'n'), (14, [0, 3, 10, 14], 0.4, 'a')],
        desc="Off-beat minor-ninth pumping stabs alternating i9 and bVII.",
        bpm=(120, 126), energy="mid", density="core",
        tags=["house", "pump", "stab"], harmonic="min9")
    chords("house", "House Breakdown Chords", "breakdown",
        [(0, [0, 3, 10, 14], 8.0, 's'), (16, [-2, 2, 9, 12], 8.0, 's')],
        desc="Two sustained minor-ninth pads for the house breakdown.",
        bpm=(120, 126), energy="low", density="sparse",
        tags=["house", "pad", "breakdown"], harmonic="min9", length=32, bars=2)

    # --- utility: cross-genre building blocks ---
    chords("utility", "Aeolian Loop", "core",
        [(0, [0, 3, 7], 8.0, 'n'), (8, [-2, 2, 5], 8.0, 'n'),
         (16, [-4, 0, 3], 8.0, 'n'), (24, [-2, 2, 5], 8.0, 'a')],
        desc="Natural-minor four-chord loop i-bVII-bVI-bVII in triads.",
        bpm=(90, 130), energy="mid", density="core",
        tags=["utility", "aeolian", "loop"], harmonic="aeolian", length=32, bars=2)
    chords("utility", "Sus2 Add9 Drift", "sparse",
        [(0, [0, 2, 7, 14], 8.0, 'n'), (8, [-2, 0, 5, 12], 8.0, 'n'),
         (16, [-4, 0, 7, 14], 8.0, 'n'), (24, [0, 2, 7, 9], 8.0, 'a')],
        desc="Open sus2/add9 progression with unresolved, airy upper voicings.",
        bpm=(90, 130), energy="low", density="sparse",
        tags=["utility", "sus2", "add9", "open"], harmonic="sus2",
        length=32, bars=2)
    chords("utility", "Maj7 Warm Bed", "dense",
        [(0, [0, 4, 7, 11], 16.0, 's'), (16, [-3, 0, 4, 9], 16.0, 's')],
        desc="Two sustained major-seventh pad chords as a warm harmonic bed (Imaj7 to vi).",
        bpm=(90, 130), energy="low", density="dense",
        tags=["utility", "pad", "warm"], harmonic="maj7", length=32, bars=2)
    chords("utility", "Min9 Neutral Vamp", "core",
        [(0, [0, 3, 10, 14], 4.0, 'n'), (8, [-2, 2, 9, 12], 4.0, 'a')],
        desc="Neutral one-bar minor-ninth vamp (i9 to bVII9) usable across genres.",
        bpm=(90, 130), energy="mid", density="core",
        tags=["utility", "vamp"], harmonic="min9")
    chords("utility", "Dom9 Turnaround", "sparse",
        [(0, [0, 4, 10, 14], 2.0, 'n'), (8, [-5, 0, 4, 7], 2.0, 'a')],
        desc="Dominant-ninth to tonic turnaround (V9 to I) for cadence endings.",
        bpm=(90, 130), energy="mid", density="sparse",
        tags=["utility", "turnaround"], harmonic="dom9")

    # ==================================================================
    # C. Performance families (role: chords)
    # ==================================================================
    family("chords-amapiano-keys", "Jazzy Keys", "chords", "amapiano",
           [("core", "Jazzy Keys"), ("sparse", "Two-Chord Vamp"),
            ("dense", "Three-Bar Jazzy Vamp")])
    family("chords-boom-bap-keys", "Dusty Keys", "chords", "boom-bap",
           [("core", "Rhodes Min7 Stabs"), ("dense", "Jazz ii-V-i")])
    family("chords-disco-strings", "String Stabs", "chords", "disco",
           [("core", "String Stabs"), ("sparse", "Four-Bar Strings"),
            ("dense", "ii-V-I Strings")])
    family("chords-melodic-techno", "Melodic Chords", "chords", "melodic-techno",
           [("core", "Four-Bar Chords"), ("sparse", "Three-Bar Progression"),
            ("breakdown", "Melodic Breakdown")])
    family("chords-reggae-skank", "Offbeat Skank", "chords", "reggae",
           [("core", "Offbeat Skank"), ("sparse", "Steppers Skank"),
            ("dense", "Bubble Organ")])
    family("chords-reggaeton-stabs", "Dark Minor Stabs", "chords", "reggaeton",
           [("core", "Dark Minor Stabs"), ("sparse", "Minor Add9 Stab")])
    family("chords-trap-keys", "Dark Trap Keys", "chords", "trap",
           [("core", "Minor Bell Stabs"), ("sparse", "Dark Add9 Stabs")])
    family("chords-funk-clav", "Clav Stabs", "chords", "funk",
           [("core", "Clav Ninth Stabs"), ("sparse", "Dom9 Clav Comp")])
    family("chords-dub-stab", "Dub Stab", "chords", "dub",
           [("core", "Minor7 Dub Stab"), ("sparse", "Dub Chord Echo")])
    family("chords-deep-house-pulse", "Deep Pulse", "chords", "deep-house",
           [("core", "Deep Min9 Pulse"), ("sparse", "Min9 Pulse Roll"),
            ("dense", "Four-Bar Pads")])
    family("chords-lo-fi-keys", "Lo-Fi Keys", "chords", "lo-fi",
           [("core", "Lo-Fi Maj9 Pad"), ("sparse", "Lo-Fi Rhodes Comp")])
    family("chords-jazz-dorian", "Dorian Vamp", "chords", "jazz",
           [("core", "Dorian Vamp"), ("sparse", "Dorian Trade")])
    family("chords-synthwave", "Neon Chords", "chords", "synthwave",
           [("core", "Neon Descent"), ("sparse", "Neon Rise")])
    family("chords-trance-lift", "Trance Lift", "chords", "trance",
           [("core", "Minor Trance Lift"), ("sparse", "Uplift Chords"),
            ("breakdown", "Trance Breakdown")])
    family("chords-ambient-pedal", "Pedal Drift", "chords", "ambient",
           [("core", "Ambient Pedal Drift"), ("sparse", "Pedal Bloom")])
    family("chords-techno-modal", "Modal Vamp", "chords", "techno",
           [("core", "Modal Techno Vamp"), ("sparse", "Hypnotic Sus Stab"),
            ("peak", "Peak Stab Vamp")])
    family("chords-house-pump", "House Pump", "chords", "house",
           [("core", "House Min9 Pump"), ("sparse", "Two-Bar Stab Vamp"),
            ("breakdown", "House Breakdown Chords")])
    family("chords-utility", "Utility Chords", "chords", "utility",
           [("core", "Aeolian Loop"), ("sparse", "Sus2 Add9 Drift"),
            ("dense", "Maj7 Warm Bed")])
    # "Min9 Neutral Vamp" and "Dom9 Turnaround" stay standalone (no family).
