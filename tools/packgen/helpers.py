"""Role-bound emit helpers + shared metadata/provenance builders."""
from engine import (emit, family, drum_steps, mel_steps, cc_slots, write_all,
                    bake_timing_into_steps, FEEL, FAMILIES, FILES, NEW_GENRES)

# Default timing template per genre (Phase 7). Swing-dependent genres bake real
# per-note microtiming; everything else stays straight. Individual calls may
# override via the `timing=` kwarg. Footwork's triplet feel is carried by ratchets
# (a real sub-step subdivision) plus light humanization.
GENRE_TIMING = {
    "boom-bap": "mpc-swing",
    "funk": "light-swing",
    "amapiano": "mpc-swing",
    "dancehall": "light-swing",
    "footwork": "humanized",
    # focused Phase-7 timing pack genres:
    "2step": "mpc-swing",
    "lo-fi-hh": "laid-back",
    "shuffle-house": "hard-swing",
    "triplet-dubstep": "triplet-shuffle",
    "jungle-breaks": "pushed",
    "glitch-idm": "humanized",
}
SEED = 1337  # fixed seed for humanized templates (deterministic, matches lint)

DRUM_DEVICES = ["t8-drums", "rd-8", "drumbrute-impact", "circuit-drums", "generic-gm-drums"]
BASS_DEVICES = ["t8-bass", "td-3", "generic-mono"]
SYNTH_DEVICES = ["s1", "j-6", "monologue", "microfreak", "minilogue-xd", "generic-poly"]


def prov(pack, refs, author="midip factory"):
    return {"source": "factory", "author": author, "pack": pack,
            "license": "original generic groove (no copyrighted material)",
            "references": refs}


def _timing(genre, timing):
    return timing if timing is not None else GENRE_TIMING.get(genre, "straight")


def drums(genre, name, function, grids, *, desc, bpm, feel, energy, density, tags,
          prov, ratch=None, prob=None, cc=None, length=16, timing=None, meter=None, spb=16, bars=1):
    steps = drum_steps(length, grids, ratch, prob)
    tmg = _timing(genre, timing)
    bake_timing_into_steps(steps, "drums", tmg, SEED)
    meta = dict(desc=desc, bpm_min=bpm[0], bpm_max=bpm[1],
                feel=(FEEL[tmg] if tmg != "straight" else feel), timing=tmg,
                energy=energy, density=density, tags=tags, harmonic=None,
                chord_poly="none", subgenre=genre, compatible_devices=DRUM_DEVICES)
    if meter:
        meta["meter"]=meter; meta["steps_per_bar"]=spb; meta["bars"]=bars
        meta["tags"]=list(meta["tags"])+[meter]
    emit("drums", genre, name, function, steps, "drums", length, meta, prov,
         cc=cc_slots(length, cc) if cc else None)
    return name


def _poly(steps):
    return "chord" if any(isinstance(s, list) for s in steps) else "mono"


def bass(genre, name, function, notes, *, desc, bpm, feel, energy, density, tags,
         prov, harmonic=None, cc=None, length=16, timing=None, meter=None, spb=16, bars=1):
    steps = mel_steps(length, notes)
    tmg = _timing(genre, timing)
    bake_timing_into_steps(steps, "melodic", tmg, SEED)
    meta = dict(desc=desc, bpm_min=bpm[0], bpm_max=bpm[1],
                feel=(FEEL[tmg] if tmg != "straight" else feel), timing=tmg,
                energy=energy, density=density, tags=tags, harmonic=harmonic,
                chord_poly=_poly(steps), subgenre=genre, compatible_devices=BASS_DEVICES)
    if meter:
        meta["meter"]=meter; meta["steps_per_bar"]=spb; meta["bars"]=bars
        meta["tags"]=list(meta["tags"])+[meter]
    emit("bass", genre, name, function, steps, "melodic", length, meta, prov,
         cc=cc_slots(length, cc) if cc else None)
    return name


def synth(genre, name, function, notes, *, desc, bpm, feel, energy, density, tags,
          prov, harmonic=None, cc=None, length=16, timing=None, meter=None, spb=16, bars=1):
    steps = mel_steps(length, notes)
    tmg = _timing(genre, timing)
    bake_timing_into_steps(steps, "melodic", tmg, SEED)
    meta = dict(desc=desc, bpm_min=bpm[0], bpm_max=bpm[1],
                feel=(FEEL[tmg] if tmg != "straight" else feel), timing=tmg,
                energy=energy, density=density, tags=tags, harmonic=harmonic,
                chord_poly=_poly(steps), subgenre=genre, compatible_devices=SYNTH_DEVICES)
    if meter:
        meta["meter"]=meter; meta["steps_per_bar"]=spb; meta["bars"]=bars
        meta["tags"]=list(meta["tags"])+[meter]
    emit("synth", genre, name, function, steps, "melodic", length, meta, prov,
         cc=cc_slots(length, cc) if cc else None)
    return name
