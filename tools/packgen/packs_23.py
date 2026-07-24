"""Pack 2 (funk / disco) + Pack 3 (reggae / dancehall)."""
from engine import row as R, family
from helpers import drums, bass, synth, prov

# ============================ PACK 2: FUNK / DISCO ============================
FK = prov("funk-disco", ["Senn et al., microtiming & groove in funk (Frontiers in Psychology 2016)",
                         "ZGMTH 'Microtiming in Early Funk' (14-groove analysis)",
                         "'On the One' downbeat emphasis (Wikipedia/MasterClass)",
                         "Magenta Groove MIDI Dataset"])
DS = prov("funk-disco", ["DRUM! Magazine four-on-the-floor disco lesson",
                         "zZounds disco drums (kick every beat, open hat on the and)",
                         "Disco BPM reference (bpmcalc)","Magenta Groove MIDI Dataset"])

def pack_funk_disco():
    # ---- funk ----
    g="funk"
    kw=dict(bpm=(96,112), feel="straight-16", tags=["funk","ghost-notes"], prov=FK)
    c=drums(g,"The One","core",
        {"BD":R(16,{0:'^',6:'x',10:'x'}),
         "SD":R(16,{4:'X',12:'X',1:'o',3:'o',7:'o',9:'o',14:'o'}),
         "CH":R(16,{i:('X' if i in(0,4,8,12) else 'x') for i in range(16)}),
         "OH":R(16,{14:'x'})},
        prob={"SD":{1:0.6,3:0.6,7:0.55,9:0.6,14:0.5},"OH":{14:0.5}},
        desc="Heavy accent on the one, backbeat on 2 & 4, and a probabilistic sixteenth ghost-snare lattice under driving hats.",
        energy="high", density="core", **kw)
    s=drums(g,"Pocket","sparse",
        {"BD":R(16,{0:'^',10:'x'}),"SD":R(16,{4:'X',12:'X'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'})},
        desc="Stripped pocket: strong one, clean backbeat, eighth hats — space for the bass.",
        energy="mid", density="sparse", **kw)
    d=drums(g,"Ghost Funk","dense",
        {"BD":R(16,{0:'^',3:'x',6:'x',10:'x',11:'x'}),
         "SD":R(16,{4:'X',12:'X',1:'o',2:'o',6:'o',7:'o',9:'o',10:'o',14:'o',15:'o'}),
         "CH":R(16,{i:('X' if i in(0,4,8,12) else 'x') for i in range(16)})},
        prob={"SD":{1:0.7,2:0.5,6:0.6,7:0.7,9:0.7,10:0.5,14:0.6,15:0.6}},
        desc="Dense ghost-note funk: a busy sixteenth snare lattice at high probability over a syncopated kick.",
        energy="high", density="dense", **kw)
    f=drums(g,"Tom Turn","fill",
        {"BD":R(16,{0:'^'}),"SD":R(16,{4:'X',13:'x',15:'x'}),
         "MT":R(16,{8:'x',9:'x'}),"HT":R(16,{10:'x',11:'x'}),"CC":R(16,{0:'X'})},
        ratch={"SD":{13:3,15:4}},
        desc="Bar-end fill: tom move and a ratcheted snare turnaround into a crash.",
        energy="high", density="dense", **kw)
    b=drums(g,"On-The-One Break","breakdown",
        {"BD":R(16,{0:'^'}),"SD":R(16,{3:'o',7:'o',11:'o',15:'o'}),
         "CH":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        prob={"SD":{3:0.6,7:0.6,11:0.6,15:0.6}},
        desc="Breakdown down to the one, ghost snares and quarter hats for a re-entry.",
        energy="low", density="sparse", **kw)
    family("funk-drums-one","On The One","drums",g,
           [("core",c),("sparse",s),("dense",d),("fill",f),("breakdown",b)])

    kw=dict(bpm=(96,112), feel="straight-16", tags=["funk","bass","syncopated"], prov=FK, harmonic="dorian")
    c=bass(g,"Syncopated Pocket","core",
        [(0,0,0.5,'a'),(3,0,0.25,'n'),(6,3,0.4,'n'),(8,0,0.5,'n'),(10,-2,0.25,'n'),(11,0,0.4,'n'),(14,5,0.4,'s')],
        desc="Staccato syncopated sixteenth bass locked to the kick, emphasising the one; Dorian colour.",
        energy="high", density="core", **kw)
    s=bass(g,"Root Stab","sparse",
        [(0,0,0.8,'a'),(6,0,0.4,'n'),(10,-2,0.6,'n')],
        desc="Spare root stabs anchoring the one and the pushes.",
        energy="mid", density="sparse", **kw)
    family("funk-bass-pocket","Syncopated Pocket","bass",g,[("core",c),("sparse",s)])

    # Clav Ninth Stabs (dominant-ninth chord) moved to role "chords" (packs_chords.py).

    # ---- disco ----
    g="disco"
    kw=dict(bpm=(115,125), feel="straight", tags=["disco","four-on-floor"], prov=DS)
    c=drums(g,"Disco Floor","core",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"SD":R(16,{4:'X',12:'X'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'}),
         "OH":R(16,{2:'x',6:'x',10:'x',14:'x'})},
        desc="Four-on-the-floor kick, backbeat on 2 & 4, closed eighth hats and the signature open hat on every off-beat.",
        energy="mid", density="core", **kw)
    s=drums(g,"Verse","sparse",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'})},
        desc="Verse groove: four-on-floor and closed eighth hats, open hats muted.",
        energy="low", density="sparse", **kw)
    d=drums(g,"Chorus","dense",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"SD":R(16,{4:'X',12:'X'}),
         "CH":R(16,{i:'x' for i in range(16)}),"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),
         "CB":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'})},
        desc="Full chorus with sixteenth hats and a Latin-disco cowbell riding the eighths.",
        energy="high", density="dense", **kw)
    f=drums(g,"Snare Crescendo","fill",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"SD":R(16,{8:'o',10:'x',12:'x',14:'X'}),"CC":R(16,{0:'X'})},
        ratch={"SD":{12:2,14:4}},
        desc="Rising sixteenth snare crescendo across the bar into a crash on the one.",
        energy="high", density="dense", **kw)
    b=drums(g,"Filtered Drop","breakdown",
        {"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),"CH":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        desc="Breakdown to open and closed hats only — kick and backbeat dropped.",
        energy="low", density="sparse", **kw)
    family("disco-drums-floor","Four-on-the-Floor","drums",g,
           [("core",c),("sparse",s),("dense",d),("fill",f),("breakdown",b)])

    kw=dict(bpm=(115,125), feel="straight", tags=["disco","octave","bass"], prov=DS, harmonic="minor")
    c=bass(g,"Octave Bass","core",
        [(0,0,0.5,'a'),(2,12,0.5,'n'),(4,0,0.5,'n'),(6,12,0.5,'n'),(8,0,0.5,'n'),(10,12,0.5,'n'),(12,0,0.5,'n'),(14,12,0.5,'n')],
        desc="The disco octave-bass engine: root on the beat, octave up on every 'and'.",
        energy="high", density="core", **kw)
    v=bass(g,"Walk-Up","dense",
        [(0,0,0.5,'a'),(2,12,0.5,'n'),(4,0,0.5,'n'),(6,12,0.5,'n'),(8,0,0.5,'n'),(10,12,0.5,'n'),(12,3,0.5,'n'),(14,5,0.5,'s')],
        desc="Octave bass with a bar-end walk-up into the next chord.",
        energy="high", density="dense", **kw)
    family("disco-bass-octave","Octave Bass","bass",g,[("core",c),("dense",v)])

    # Disco string chord stabs (String Stabs, ii-V-I Strings) moved to role "chords".


# ============================ PACK 3: REGGAE / DANCEHALL ======================
RG = prov("reggae-dancehall", ["'One drop rhythm' (Wikipedia): empty beat 1, snare/kick on 3; steppers; rockers",
                              "Reggae drumming: cross-stick rimshot & timbale snare (Bax Music)",
                              "Foundational reggae drums (zZounds)","Reggae BPM range (bpmcalc)"])
DH = prov("reggae-dancehall", ["'Dembow: A Loop History' (Red Bull Music Academy)",
                              "Dancehall dembow riddim lineage","Magenta Groove MIDI Dataset"])

def pack_reggae_dancehall():
    # ---- reggae ----
    g="reggae"
    kw=dict(bpm=(70,90), feel="half-time", tags=["reggae","one-drop"], prov=RG)
    c=drums(g,"One Drop","core",
        {"BD":R(16,{8:'X'}),"RS":R(16,{8:'X'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'})},
        desc="Classic one drop: beat 1 deliberately empty, kick and cross-stick together on beat 3, steady eighth hats.",
        energy="low", density="core", **kw)
    s=drums(g,"One Drop Bare","sparse",
        {"BD":R(16,{8:'X'}),"RS":R(16,{8:'X'}),"CH":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        desc="Barest one drop: the drop on 3 with quarter hats only.",
        energy="low", density="sparse", **kw)
    f=drums(g,"Rimshot Roll","fill",
        {"BD":R(16,{8:'X'}),"RS":R(16,{12:'x',13:'x',14:'x',15:'x'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x'}),"CC":R(16,{0:'X'})},
        ratch={"RS":{13:2,14:3,15:4}},
        desc="Cross-stick ratchet roll across the last beat, resolving to a crash on the one.",
        energy="mid", density="dense", **kw)
    b=drums(g,"Dub Space","breakdown",
        {"BD":R(16,{8:'X'}),"CH":R(16,{4:'x',12:'x'})},
        desc="Dub breakdown: just the drop and two ticking hats, leaving space for echo.",
        energy="low", density="sparse", **kw)
    family("reggae-drums-onedrop","One Drop","drums",g,
           [("core",c),("sparse",s),("fill",f),("breakdown",b)])

    kw=dict(bpm=(70,92), feel="half-time", tags=["reggae","steppers"], prov=RG)
    c=drums(g,"Steppers","core",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"RS":R(16,{8:'X'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'})},
        desc="Militant steppers: four-on-the-floor kick with the cross-stick backbeat still on 3.",
        energy="mid", density="core", **kw)
    d=drums(g,"Tom Militant","dense",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"RS":R(16,{8:'X'}),
         "CH":R(16,{i:'x' for i in range(16)}),"MT":R(16,{11:'x'}),"HT":R(16,{13:'x',15:'x'})},
        prob={"MT":{11:0.5},"HT":{13:0.5,15:0.5}},
        desc="Driving steppers with sixteenth hats and probabilistic tom accents toward the bar end.",
        energy="high", density="dense", **kw)
    family("reggae-drums-steppers","Steppers","drums",g,[("core",c),("dense",d)])

    kw=dict(bpm=(70,90), feel="half-time", tags=["reggae","bass","riddim"], prov=RG, harmonic="minor")
    c=bass(g,"Riddim Bass","core",
        [(8,0,1.0,'a'),(10,-2,0.5,'n'),(12,-4,0.5,'n'),(14,-5,0.75,'n')],
        desc="Dubby lead bass resting on beat 1 with the drop, then a syncopated melodic phrase into the next bar.",
        energy="mid", density="core", **kw)
    s=bass(g,"Root Anchor","sparse",
        [(8,0,2.0,'a'),(14,-5,1.0,'n')],
        desc="Two round root notes anchoring the riddim.",
        energy="low", density="sparse", **kw)
    family("reggae-bass-riddim","Riddim Bass","bass",g,[("core",c),("sparse",s)])

    # Reggae organ skank chords (Offbeat Skank, Bubble Organ) moved to role "chords".

    # ---- dancehall ----
    g="dancehall"
    kw=dict(bpm=(88,105), feel="straight-16", tags=["dancehall","dembow"], prov=DH)
    c=drums(g,"Dembow Riddim","core",
        {"BD":R(16,{0:'X',8:'x',10:'o'}),
         "SD":R(16,{3:'x',6:'X',11:'x',14:'X'}),
         "CH":R(16,{2:'x',6:'x',10:'x',14:'x'})},
        prob={"BD":{10:0.5}},
        desc="The dancehall dembow riddim: sparse kick, the boom-ch-boom-chick snare figure, offbeat hat tick.",
        energy="mid", density="core", **kw)
    s=drums(g,"Dembow Sparse","sparse",
        {"BD":R(16,{0:'X',8:'x'}),"SD":R(16,{6:'X',14:'X'}),"CH":R(16,{2:'x',6:'x',10:'x',14:'x'})},
        desc="Stripped dembow: kick, the two main snare hits and the offbeat hat.",
        energy="low", density="sparse", **kw)
    d=drums(g,"Dembow Roll","dense",
        {"BD":R(16,{0:'X',8:'x',10:'o'}),"SD":R(16,{3:'x',6:'X',11:'x',14:'X'}),
         "CH":R(16,{i:'x' for i in range(16)}),"RS":R(16,{12:'x',13:'x'})},
        ratch={"RS":{13:2},"CH":{15:2}},
        desc="Busier dembow with sixteenth hats and a rim ratchet lead-in.",
        energy="high", density="dense", **kw)
    family("dancehall-drums-dembow","Dembow Riddim","drums",g,
           [("core",c),("sparse",s),("dense",d)])

    kw=dict(bpm=(88,105), feel="straight-16", tags=["dancehall","bass","dembow"], prov=DH, harmonic="minor")
    c=bass(g,"Dembow Bass","core",
        [(0,0,0.75,'a'),(8,0,0.5,'n'),(11,-2,0.5,'n'),(14,-4,0.5,'s')],
        desc="Punchy dembow bass tracking the kick with a short melodic tail.",
        energy="mid", density="core", **kw)
    family("dancehall-bass-dembow","Dembow Bass","bass",g,[("core",c)])
