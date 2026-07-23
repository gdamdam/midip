#!/usr/bin/env python3
import sys, os
sys.path.insert(0, os.path.dirname(__file__))
from engine import FILES, FAMILIES, write_all, NEW_GENRES
from packs_16 import pack_hiphop, pack_club
from packs_23 import pack_funk_disco, pack_reggae_dancehall
from packs_45 import pack_afro_amapiano, pack_reggaeton_baile
from packs_timing import pack_timing
from packs_meter import pack_meter

for fn in (pack_hiphop, pack_club, pack_funk_disco, pack_reggae_dancehall,
           pack_afro_amapiano, pack_reggaeton_baile, pack_timing, pack_meter):
    fn()

nf, nfam = write_all()
print(f"wrote {nf} v2 pattern files, {nfam} families across genres: {sorted(NEW_GENRES)}")

# quick breakdown by role/genre/function
from collections import Counter
byrole = Counter(); bygenre = Counter(); byfunc = Counter()
import json
for js in FILES.values():
    o = json.loads(js)
    byrole[o["role"]] += 1
    bygenre[o["genre"]] += 1
for fam in FAMILIES:
    for m in fam["members"]:
        byfunc[m["function"]] += 1
print("by role:", dict(byrole))
print("by genre:", dict(sorted(bygenre.items())))
print("by function:", dict(byfunc))
