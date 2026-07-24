"""Pack 4 (afro-house / amapiano) + Pack 5 (reggaeton / dembow / baile funk)."""
from engine import row as R, family
from helpers import drums, bass, synth, prov

# ============================ PACK 4: AFRO HOUSE / AMAPIANO ===================
AF = prov("afro-house-amapiano", ["Afro House production 101 (Native Instruments)",
                                  "Afro House BPM & structure references (vibesdj)",
                                  "Generic son-style bell timeline (public-domain rhythmic template)",
                                  "Magenta Groove MIDI Dataset"])
AM = prov("afro-house-amapiano", ["'Amapiano' (Wikipedia): Gauteng township origins, kwaito/deep-house/jazz lineage, log drum",
                                 "Amapiano's Second Wave — South African history (Mixmag)",
                                 "Amapiano log-drum production (InspiredByBeatz)",
                                 "Amapiano BPM ~108-118 (vibesdj)"])

def pack_afro_amapiano():
    # ---- afro-house ----
    g="afro-house"
    kw=dict(bpm=(118,125), feel="straight", tags=["afro-house","percussive"], prov=AF)
    c=drums(g,"Afro Groove","core",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),
         "CH":R(16,{i:'x' for i in range(16)}),"CB":R(16,{0:'x',3:'x',6:'x',10:'x',12:'x'}),
         "RS":R(16,{2:'x',9:'x',13:'x'})},
        prob={"CH":{1:0.7,3:0.7,5:0.7,7:0.7,9:0.7,11:0.7,13:0.7,15:0.7},"RS":{2:0.6,9:0.6,13:0.6}},
        desc="Four-on-the-floor with offbeat open hats, a son-style cowbell timeline and a syncopated rim layer standing in for congas.",
        energy="mid", density="core", **kw)
    s=drums(g,"Afro Intro","sparse",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"CH":R(16,{i:'x' for i in range(16)})},
        prob={"CH":{1:0.6,3:0.6,5:0.6,7:0.6,9:0.6,11:0.6,13:0.6,15:0.6}},
        desc="Intro: four-on-floor and probabilistic sixteenth hats only, timeline muted.",
        energy="low", density="sparse", **kw)
    d=drums(g,"Afro Perc","dense",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),
         "CH":R(16,{i:'x' for i in range(16)}),"CB":R(16,{0:'x',3:'x',6:'x',10:'x',12:'x'}),
         "RS":R(16,{1:'x',5:'x',9:'x',13:'x'}),"MT":R(16,{6:'x',14:'x'}),"HT":R(16,{7:'x',15:'x'})},
        desc="Full percussion layer: cowbell timeline, rim and two-tom polyrhythm (device-neutral stand-ins for hand percussion).",
        energy="high", density="dense", **kw)
    f=drums(g,"Afro Fill","fill",
        {"BD":R(16,{0:'X',4:'X',8:'X'}),"MT":R(16,{9:'x',11:'x'}),"HT":R(16,{10:'x',13:'x',15:'x'}),"CC":R(16,{0:'X'})},
        ratch={"HT":{15:3}},
        desc="Tom cascade fill with a ratcheted lead-in and a crash on the one.",
        energy="high", density="dense", **kw)
    family("afro-house-drums-groove","Afro Groove","drums",g,
           [("core",c),("sparse",s),("dense",d),("fill",f)])

    kw=dict(bpm=(118,125), feel="straight", tags=["afro-house","bass","rolling"], prov=AF, harmonic="minor-pentatonic")
    c=bass(g,"Rolling Pentatonic","core",
        [(0,0,0.5,'a'),(4,3,0.5,'n'),(6,7,0.5,'n'),(8,0,0.5,'n'),(11,10,0.5,'n'),(14,7,0.5,'s')],
        desc="Hypnotic rolling minor-pentatonic bass sitting under the four-on-floor.",
        energy="mid", density="core", **kw)
    family("afro-house-bass-roll","Rolling Pentatonic","bass",g,[("core",c)])

    # Deep Pad Vamp (minor-ninth chord pad) moved to role "chords" (packs_chords.py).

    # ---- amapiano ----
    g="amapiano"
    kw=dict(bpm=(108,118), feel="straight-16", tags=["amapiano","log-drum"], prov=AM)
    c=drums(g,"Piano Core","core",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"CH":R(16,{i:'x' for i in range(16)}),
         "OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),"SD":R(16,{12:'X',13:'o',14:'o'})},
        prob={"CH":{1:0.75,3:0.75,5:0.75,7:0.75,9:0.75,11:0.75,13:0.75,15:0.75},
              "RS":{},"SD":{13:0.6,14:0.5}},
        desc="Four-on-floor with rolling sixteenth hats, offbeat open hats and a late snare backbeat with ghost tail on beat 4.",
        energy="mid", density="core", **kw)
    s=drums(g,"Log Drum Bed","sparse",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"CH":R(16,{i:'x' for i in range(16)}),
         "OH":R(16,{6:'x',14:'x'})},
        desc="Sparse bed for the log drum: four-on-floor with rolling sixteenth hats and a light offbeat open hat.",
        energy="low", density="sparse", **kw)
    d=drums(g,"Piano Dense","dense",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"CH":R(16,{i:'x' for i in range(16)}),
         "OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),"SD":R(16,{12:'X'}),
         "RS":R(16,{3:'x',6:'x',11:'x',15:'x'})},
        prob={"RS":{3:0.7,6:0.7,11:0.7,15:0.7}},
        desc="Denser groove adding a probabilistic rim layer for the busy shuffle-style percussion.",
        energy="high", density="dense", **kw)
    b=drums(g,"Piano Break","breakdown",
        {"CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'}),"OH":R(16,{6:'x',14:'x'})},
        desc="Breakdown for the jazzy chords and log drum: hats only, kick dropped.",
        energy="low", density="sparse", **kw)
    family("amapiano-drums-core","Piano Core","drums",g,
           [("core",c),("sparse",s),("dense",d),("breakdown",b)])

    # log drum on the BASS lane — a pitched instrument, the correct placement (not an approximation)
    kw=dict(bpm=(108,118), feel="straight-16", tags=["amapiano","log-drum","bass"], prov=AM, harmonic="minor")
    c=bass(g,"Log Drum","core",
        [(3,0,1.0,'a','slide'),(6,3,1.25,'n','slide'),(10,-2,1.0,'n','slide'),(13,0,1.25,'a','slide')],
        desc="The amapiano log drum: a deep, syncopated, gliding pitched bass figure landing in the kick gaps.",
        energy="mid", density="core", **kw)
    d=bass(g,"Log Drum Double","dense",
        [(3,0,0.5,'a','slide'),(4,0,0.5,'n'),(6,3,0.75,'n','slide'),(8,-2,0.5,'n'),
         (10,-2,0.75,'n','slide'),(13,0,0.5,'a','slide'),(14,3,0.5,'n','slide')],
        desc="Double-time log-drum phrasing with more glides and call-and-response with the kick.",
        energy="high", density="dense", **kw)
    family("amapiano-bass-logdrum","Log Drum","bass",g,[("core",c),("dense",d)])

    # Amapiano jazzy keys chords (Jazzy Keys, Two-Chord Vamp) moved to role "chords".


# ==================== PACK 5: REGGAETON / DEMBOW / BAILE FUNK =================
RT = prov("reggaeton-dembow-baile", ["'Dembow beat' (Wikipedia): rhythm structure, dancehall->reggaeton lineage",
                                     "'Dembow Explained' (Berklee)",
                                     "3+3+2 / habanera shared rhythmic ancestor (Wayne Marshall commentary)",
                                     "Reggaeton programming (Native Instruments / MusicRadar)"])
BF = prov("reggaeton-dembow-baile", ["'Funk carioca' (Wikipedia) & 'Tamborzao' (Rate Your Music): origins, tempo",
                                     "How to Make Brazilian Funk (Loopcloud) — 'do not over-quantize'",
                                     "Magenta Groove MIDI Dataset"])

def pack_reggaeton_baile():
    # ---- reggaeton ----
    g="reggaeton"
    kw=dict(bpm=(88,100), feel="straight-16", tags=["reggaeton","dembow"], prov=RT)
    c=drums(g,"Dembow Core","core",
        {"BD":R(16,{0:'X',4:'x',8:'X',12:'x'}),
         "SD":R(16,{3:'x',6:'X',11:'x',14:'X'}),
         "CH":R(16,{2:'x',6:'x',10:'x',14:'x'})},
        desc="The reggaeton dembow: kick on the quarters with the canonical snare figure on the sixteenths (boom-ch-boom-chick).",
        energy="mid", density="core", **kw)
    s=drums(g,"Half-Drop","sparse",
        {"BD":R(16,{0:'X',8:'X'}),"SD":R(16,{6:'X',14:'X'}),"CH":R(16,{2:'x',6:'x',10:'x',14:'x'})},
        desc="Spacious half-drop: kick on 1 & 3, the two main snares and offbeat hats.",
        energy="low", density="sparse", **kw)
    d=drums(g,"Tresillo Drive","dense",
        {"BD":R(16,{0:'X',6:'x',10:'x'}),"SD":R(16,{3:'x',6:'X',11:'x',14:'X'}),
         "CH":R(16,{i:'x' for i in range(16)}),"OH":R(16,{6:'x',14:'x'})},
        desc="Tresillo (3+3+2) kick under a busy dembow snare and driving sixteenth hats.",
        energy="high", density="dense", **kw)
    f=drums(g,"Perreo Roll","fill",
        {"BD":R(16,{0:'X',8:'X'}),"SD":R(16,{12:'x',13:'x',14:'x',15:'X'}),"CC":R(16,{0:'X'})},
        ratch={"SD":{12:2,13:3,14:4,15:6}},
        desc="Building snare ratchet roll across the last beat into a crash on the one.",
        energy="high", density="dense", **kw)
    family("reggaeton-drums-dembow","Dembow Core","drums",g,
           [("core",c),("sparse",s),("dense",d),("fill",f)])

    kw=dict(bpm=(88,100), feel="straight-16", tags=["reggaeton","bass","sub"], prov=RT, harmonic="minor")
    c=bass(g,"Dembow Sub","core",
        [(0,0,0.75,'a'),(8,0,0.75,'n'),(11,-2,0.5,'n'),(14,-4,0.5,'n')],
        desc="Sub bass locked to the kick with a short minor tail into the turnaround.",
        energy="mid", density="core", **kw)
    family("reggaeton-bass-sub","Dembow Sub","bass",g,[("core",c)])

    # Dark Minor Stabs (minor triad chords) moved to role "chords" (packs_chords.py).

    # ---- baile funk (tamborzao) — honest kick+snare skeleton, NOT authentic atabaque ----
    g="baile-funk"
    kw=dict(bpm=(128,140), feel="straight-16", tags=["baile-funk","tamborzao","brazilian"], prov=BF)
    c=drums(g,"Tamborzao Skeleton","core",
        {"BD":R(16,{0:'X',10:'x'}),"SD":R(16,{3:'x',6:'x',12:'x'}),
         "CH":R(16,{2:'x',6:'x',10:'x',14:'x'})},
        prob={"CH":{2:0.7,6:0.7,10:0.7,14:0.7}},
        desc="Kick-and-snare skeleton of the tamborzao 'tum-tcha-tcha' figure (device-neutral voices — not authentic atabaque/surdo samples).",
        energy="mid", density="core", **kw)
    d=drums(g,"Mandelao Heavy","dense",
        {"BD":R(16,{0:'X',3:'x',6:'x',10:'x',13:'x'}),"SD":R(16,{4:'X',12:'X'}),
         "CB":R(16,{2:'x',6:'x',10:'x',14:'x'}),"CH":R(16,{i:'x' for i in range(16)})},
        desc="Busy syncopated kick pattern with a hard backbeat and a metallic cowbell tick — heavier modern variant.",
        energy="high", density="dense", **kw)
    f=drums(g,"Tamborzao Roll","fill",
        {"BD":R(16,{0:'X',10:'x'}),"SD":R(16,{12:'x',13:'x',14:'x',15:'x'}),"CC":R(16,{0:'X'})},
        ratch={"SD":{13:2,14:4,15:6}},
        desc="Snare ratchet roll across the last beat into a crash.",
        energy="high", density="dense", **kw)
    family("baile-funk-drums-tamborzao","Tamborzao Skeleton","drums",g,
           [("core",c),("dense",d),("fill",f)])

    kw=dict(bpm=(128,140), feel="straight-16", tags=["baile-funk","bass","808"], prov=BF, harmonic="minor")
    c=bass(g,"Gliding 808","core",
        [(0,0,1.5,'a'),(6,0,0.5,'n'),(10,-4,1.5,'n','slide'),(13,-2,0.75,'n','slide')],
        desc="808-style sub that glides between roots, reinforcing the low 'tum'.",
        energy="mid", density="core", **kw)
    family("baile-funk-bass-808","Gliding 808","bass",g,[("core",c)])
