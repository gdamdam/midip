#!/usr/bin/env python3
"""Phase 6 factory-pack generator engine.

Emits v2 factory pattern files (assets/patterns/v2/*.json) + family registry
entries. Uses ONLY features that are audible/correct in the current engine:
velocity dynamics, ratchets (real sub-step rolls), probability, real note
lengths, chords, CC locks. micro=0 (pattern microtiming/swing is deferred to
Phase 7, which fixes the engine's micro unit). Descriptions must NOT claim
swing/triplet-feel — only structural facts.

T-8 drum palette (the ONLY legal drum notes):
  BD36 RS37 SD38 CH42 OH46 MT47 CC49 HT50 RC51 CB56
"""
import json, os

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
V2DIR = os.path.join(REPO, "assets/patterns/v2")

VOICE = {"BD":36,"RS":37,"SD":38,"CH":42,"OH":46,"MT":47,"CC":49,"HT":50,"RC":51,"CB":56}

# Drum velocity bands for grid glyphs.
VEL = {"X":118, "x":102, "o":48, "^":127}   # ^ = hard accent, o = ghost
# Melodic velocity multipliers (0.5..=1.3).
MVEL = {"a":1.15, "n":1.0, "s":0.82}

# Accumulators
FAMILIES = []          # catalog family dicts
FILES = {}             # filename -> json string
NEW_GENRES = set()     # genres this generator owns (for idempotent catalog merge)

FUNC = {"core":"core","sparse":"variation_a","dense":"variation_b",
        "fill":"fill","breakdown":"breakdown","peak":"peak"}


def slug(s):
    out=[]; dash=False
    for c in s.lower():
        if c.isalnum(): out.append(c); dash=False
        elif not dash and out: out.append("-"); dash=True
    return "".join(out).strip("-")


# --- timing templates: MUST match src/pattern/timing.rs exactly -------------
def timing_offset_permille(template, i, seed=0):
    odd = (i % 2) == 1
    if template == "straight":       return 0
    if template == "light-swing":    return 80 if odd else 0
    if template == "mpc-swing":      return 160 if odd else 0
    if template == "hard-swing":     return 280 if odd else 0
    if template == "triplet-shuffle":return 333 if odd else 0
    if template == "laid-back":      return 100 if i in (4,12) else 0
    if template == "pushed":         return -60 if i in (2,6,10,14) else 0
    if template == "humanized":
        h = (((i + 1) * 2654435761) ^ seed) & 0xFFFFFFFF
        h ^= h >> 13
        lo = h & 0xFFFF
        return lo * 100 // 65535 - 50
    raise ValueError(f"unknown timing template {template}")

# Human-readable feel string per template (recorded in metadata.feel).
FEEL = {
    "straight":"straight","light-swing":"swung-16 (light 54%)","mpc-swing":"swung-16 (mpc 58%)",
    "hard-swing":"swung-16 (hard 64%)","triplet-shuffle":"triplet-shuffle feel",
    "laid-back":"laid-back (dragged backbeat)","pushed":"pushed (early offbeats)",
    "humanized":"humanized microtiming",
}

def bake_timing_into_steps(steps, kind, template, seed=0):
    """Set per-note/hit micro from the template, by step index. Mutates in place."""
    if template == "straight":
        return
    if kind == "drums":
        for i, step in enumerate(steps):
            off = timing_offset_permille(template, i, seed)
            for hit in step:
                if off: hit["micro"] = off
    else:
        for i, step in enumerate(steps):
            off = timing_offset_permille(template, i, seed)
            if step is None or not off: continue
            notes = step if isinstance(step, list) else [step]
            for n in notes:
                n["micro"] = off


def row(length, hits):
    """Build a grid string. hits: {step_index(0-based): glyph}."""
    s = ["."] * length
    for k, v in hits.items():
        s[k] = v
    return "".join(s)


def drum_steps(length, grids, ratchets=None, probs=None):
    """grids: {voice: glyphstring len==length}. ratchets/probs: {voice:{step:val}}."""
    ratchets = ratchets or {}
    probs = probs or {}
    steps = [[] for _ in range(length)]
    for v, g in grids.items():
        assert v in VOICE, f"illegal voice {v}"
        assert len(g) == length, f"{v} grid len {len(g)} != {length}"
        for i, ch in enumerate(g):
            if ch == ".": continue
            hit = {"note": VOICE[v], "vel": VEL[ch]}
            r = ratchets.get(v, {}).get(i)
            if r: hit["ratchet"] = r
            p = probs.get(v, {}).get(i)
            if p is not None: hit["prob"] = p
            steps[i].append(hit)
    return steps


def mel_steps(length, notes):
    """notes: list of (step, semis|[semis], len, kind) where kind in a/n/s + optional
    ('slide',) marker. Returns v2 melodic step array (null / obj / [obj,...])."""
    steps = [None]*length
    for spec in notes:
        step, semis, ln, kind = spec[0], spec[1], spec[2], spec[3]
        slide = len(spec) > 4 and "slide" in spec[4:]
        prob = None
        for e in spec[4:]:
            if isinstance(e, tuple) and e[0]=="prob": prob=e[1]
        if isinstance(semis, int): semis=[semis]
        objs=[]
        for s in semis:
            o={"semi":s,"vel":MVEL[kind],"slide":slide,"len":ln}
            if prob is not None: o["prob"]=prob
            objs.append(o)
        steps[step] = objs[0] if len(objs)==1 else objs
    return steps


def cc_slots(length, cc_map):
    """cc_map: {step:[(cc,val),...]}. Returns cc array trimmed of trailing empties."""
    if not cc_map: return None
    slots=[[] for _ in range(length)]
    for st,locks in cc_map.items():
        slots[st]=[{"cc":c,"val":v} for c,v in locks]
    while slots and not slots[-1]: slots.pop()
    return slots or None


def emit(role, genre, name, function, data_steps, kind, length, meta, prov, cc=None):
    NEW_GENRES.add(genre)
    fid = f"{role}.{genre}.{slug(name)}"
    obj = {"schema":"midip.pattern","version":2,"factory_id":fid,
           "role":role,"kind":kind,"genre":genre,"name":name,
           "desc":meta.pop("desc"),"length":length,"steps":data_steps}
    if cc: obj["cc"]=cc
    obj["metadata"]=meta
    obj["provenance"]=prov
    fn=f"{role}-{genre}-{slug(name)}.json"
    assert fn not in FILES, f"dup file {fn}"
    FILES[fn]=json.dumps(obj, indent=2)
    return name


def family(fid, label, role, genre, members):
    """members: list of (function_word, pattern_name)."""
    seen=set()
    fm=[]
    for fword, nm in members:
        f=FUNC[fword]
        assert f not in seen, f"family {fid} dup function {f}"
        seen.add(f); fm.append({"function":f,"name":nm})
    assert "core" in seen, f"family {fid} missing core"
    FAMILIES.append({"id":fid,"label":label,"role":role,"genre":genre,"members":fm})


def write_all():
    os.makedirs(V2DIR, exist_ok=True)
    # clean previously-generated pack files for our genres
    for f in os.listdir(V2DIR):
        if f.endswith(".json"):
            os.remove(os.path.join(V2DIR, f))
    for fn, js in FILES.items():
        open(os.path.join(V2DIR, fn), "w").write(js)
    # merge families into catalog.json (idempotent: drop families whose genre is ours)
    cat_path=os.path.join(REPO,"assets/patterns/catalog.json")
    cat=json.load(open(cat_path))
    base=[fm for fm in cat.get("families",[]) if fm["genre"] not in NEW_GENRES]
    cat["families"]=base+FAMILIES
    json.dump(cat, open(cat_path,"w"), ensure_ascii=True, separators=(",",":"))
    return len(FILES), len(FAMILIES)
