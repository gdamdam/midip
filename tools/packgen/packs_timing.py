"""Phase 7 focused timing pack: genres whose identity is groove/feel, each
authored with a real baked microtiming template (see GENRE_TIMING in helpers)."""
from engine import row as R, family
from helpers import drums, bass, synth, prov

REF = ["Magenta Groove MIDI Dataset (microtiming/groove methodology)",
       "MPC/Akai swing workflow references",
       "documented genre groove references"]

def pack_timing():
    # ---- garage / 2-step (mpc-swing) ----
    g="2step"; P=prov("timing", REF)
    kw=dict(bpm=(130,138), feel="", tags=["garage","2-step","uk-garage","swing"], prov=P)
    c=drums(g,"2-Step Skip","core",
        {"BD":R(16,{0:'X',6:'x'}),"SD":R(16,{4:'X',12:'X'}),
         "CH":R(16,{2:'x',6:'x',10:'x',14:'x'}),"OH":R(16,{14:'x'})},
        desc="Skippy UK 2-step: kick on 1 and the and-of-2, snare backbeat, offbeat hats — carries a real MPC 58% swing.",
        energy="mid", density="core", **kw)
    s=drums(g,"2-Step Sparse","sparse",
        {"BD":R(16,{0:'X',6:'x'}),"SD":R(16,{4:'X',12:'X'}),"CH":R(16,{2:'x',10:'x'})},
        desc="Stripped 2-step with swung offbeat hats.",
        energy="low", density="sparse", **kw)
    family("timing-2step-drums","UK 2-Step","drums",g,[("core",c),("sparse",s)])
    cb=bass(g,"2-Step Sub","core",
        [(0,0,0.5,'a'),(6,0,0.4,'n'),(11,-2,0.5,'n'),(14,3,0.4,'s')],
        desc="Skippy sub bass under the 2-step, swung with the hats.",
        energy="mid", density="core", harmonic="minor", **kw)
    family("timing-2step-bass","2-Step Sub","bass",g,[("core",cb)])

    # ---- hip-hop / lo-fi (laid-back) ----
    g="lo-fi-hh"; P=prov("timing", REF)
    kw=dict(bpm=(80,92), feel="", tags=["lo-fi","hip-hop","laid-back","dusty"], prov=P)
    c=drums(g,"Lo-Fi Head-Nod","core",
        {"BD":R(16,{0:'X',6:'x',10:'x'}),"SD":R(16,{4:'X',12:'X'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'})},
        desc="Dusty lo-fi head-nodder with the snare backbeat dragged deliberately late (laid-back microtiming); kicks stay on the grid.",
        energy="low", density="core", **kw)
    family("timing-lofi-drums","Lo-Fi Head-Nod","drums",g,[("core",c)])

    # ---- shuffle house (hard-swing) ----
    g="shuffle-house"; P=prov("timing", REF)
    kw=dict(bpm=(122,126), feel="", tags=["house","shuffle","swing"], prov=P)
    c=drums(g,"Shuffle House","core",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),
         "CH":R(16,{i:'x' for i in range(16)}),"RS":R(16,{4:'x',12:'x'})},
        desc="Four-on-the-floor house with heavily shuffled sixteenth hats (hard 64% swing) and an offbeat open hat.",
        energy="mid", density="core", **kw)
    family("timing-shuffle-house-drums","Shuffle House","drums",g,[("core",c)])

    # ---- triplet dubstep (triplet-shuffle) ----
    g="triplet-dubstep"; P=prov("timing", REF)
    kw=dict(bpm=(138,142), feel="", tags=["dubstep","triplet","half-time"], prov=P)
    c=drums(g,"Triplet Dub","core",
        {"BD":R(16,{0:'X',10:'x'}),"SD":R(16,{8:'X'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'})},
        ratch={"CH":{14:3,15:3}},
        desc="Half-time dubstep with a triplet-shuffle hat feel (offbeats pulled to the triplet position) plus triplet-ratchet rolls.",
        energy="high", density="core", **kw)
    family("timing-triplet-dubstep-drums","Triplet Dub","drums",g,[("core",c)])

    # ---- jungle / breaks (pushed) ----
    g="jungle-breaks"; P=prov("timing", REF)
    kw=dict(bpm=(160,174), feel="", tags=["jungle","breaks","amen","pushed"], prov=P)
    c=drums(g,"Break Roller","core",
        {"BD":R(16,{0:'X',10:'x'}),"SD":R(16,{4:'X',12:'X',7:'o',15:'o'}),
         "CH":R(16,{i:'x' for i in range(16)})},
        prob={"SD":{7:0.6,15:0.6}},
        desc="Chopped break with kick, backbeat and ghost snares, offbeat hats pushed slightly early for forward drive.",
        energy="high", density="core", **kw)
    s=drums(g,"Break Sparse","sparse",
        {"BD":R(16,{0:'X',10:'x'}),"SD":R(16,{4:'X',12:'X'}),"CH":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        desc="Sparser break skeleton with pushed offbeat hats.",
        energy="mid", density="sparse", **kw)
    family("timing-jungle-breaks-drums","Break Roller","drums",g,[("core",c),("sparse",s)])

    # ---- glitch / IDM (humanized) ----
    g="glitch-idm"; P=prov("timing", REF)
    kw=dict(bpm=(90,160), feel="", tags=["glitch","idm","humanized","scatter"], prov=P)
    c=drums(g,"Scatter","core",
        {"BD":R(16,{0:'X',3:'x',9:'x'}),"SD":R(16,{4:'X',11:'x',13:'o'}),
         "RS":R(16,{2:'x',7:'x',14:'x'}),"CH":R(16,{i:'x' for i in range(16)})},
        prob={"RS":{2:0.7,7:0.6,14:0.7},"SD":{13:0.5}},
        desc="Scattered IDM kit with probabilistic rim hits and deterministic per-step humanized microtiming (seeded jitter).",
        energy="mid", density="dense", **kw)
    family("timing-glitch-idm-drums","Scatter","drums",g,[("core",c)])
