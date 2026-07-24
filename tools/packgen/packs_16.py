"""Pack 1 (hip-hop / trap) + Pack 6 (modern club)."""
from engine import row, family
from helpers import drums, bass, synth, prov

R = row
def P(*names): return list(names)

# ============================ PACK 1: HIP-HOP ============================
BB = prov("hip-hop", ["Magenta Groove MIDI Dataset (groove/microtiming methodology)",
                      "Gillick et al. 2019 'Learning to Groove' arXiv:1905.06118",
                      "MPC classic boom-bap swing workflow references",
                      "Roland TR-909 kick heritage (Wikipedia)"])
TR = prov("hip-hop", ["Magenta Groove MIDI Dataset",
                      "Roland T-8 AIRA Compact voice/engine (Perfect Circuit)",
                      "Roland TR-808 heritage (Wikipedia)",
                      "hi-hat roll / trap programming references"])

def pack_hiphop():
    g = "boom-bap"
    # --- drums: Dusty Boom-Bap ---
    kw = dict(bpm=(82,98), feel="straight-16", tags=["hip-hop","boom-bap"], prov=BB)
    c = drums(g,"Dusty Core","core",
        {"BD":R(16,{0:'X',6:'x',10:'x',7:'o'}),
         "SD":R(16,{4:'X',12:'X',3:'o',7:'o',11:'o',15:'o'}),
         "CH":R(16,{0:'X',2:'x',4:'x',6:'x',8:'X',10:'x',12:'x',14:'x'}),
         "OH":R(16,{14:'x'})},
        prob={"BD":{7:0.3},"SD":{3:0.5,7:0.5,11:0.4,15:0.5},"OH":{14:0.4}},
        desc="Kick on 1, the and-of-2 and and-of-3; snare backbeat on 2 & 4 with probabilistic ghost notes; eighth-note hats.",
        energy="mid", density="core", **kw)
    s = drums(g,"Head-Nod Sparse","sparse",
        {"BD":R(16,{0:'X',6:'x'}),"SD":R(16,{4:'X',12:'X'}),
         "CH":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        desc="Stripped head-nod: kick, backbeat and quarter-note hats only.",
        energy="low", density="sparse", **kw)
    d = drums(g,"Ghost Lattice","dense",
        {"BD":R(16,{0:'X',6:'x',10:'x',13:'x'}),
         "SD":R(16,{4:'X',12:'X',1:'o',3:'o',7:'o',9:'o',11:'o',15:'o'}),
         "CH":R(16,{i:('X' if i in(0,8) else 'x') for i in range(16)})},
        prob={"SD":{1:0.6,3:0.7,7:0.6,9:0.6,11:0.7,15:0.7}},
        desc="Dense sixteenth-note hats with a busy probabilistic ghost-snare lattice around the backbeat.",
        energy="high", density="dense", **kw)
    f = drums(g,"Turnaround Fill","fill",
        {"BD":R(16,{0:'X'}),"SD":R(16,{12:'x',14:'x'}),
         "MT":R(16,{8:'x',9:'x'}),"HT":R(16,{10:'x',11:'x'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x'}),"CC":R(16,{0:'X'})},
        ratch={"SD":{12:3,14:4}},
        desc="Bar-end turnaround: descending tom move with a ratcheted snare roll and a crash on the downbeat.",
        energy="high", density="dense", **kw)
    b = drums(g,"Stripped Nod","breakdown",
        {"BD":R(16,{0:'X',6:'x',10:'x'}),
         "CH":R(16,{0:'x',2:'x',4:'x',6:'x',8:'x',10:'x',12:'x',14:'x'})},
        desc="Breakdown: kick and eighth-note hats only, backbeat dropped for tension.",
        energy="low", density="sparse", **kw)
    family("boom-bap-drums-dusty","Dusty Boom-Bap","drums",g,
           [("core",c),("sparse",s),("dense",d),("fill",f),("breakdown",b)])

    # --- bass: Upright Walk (A natural minor, root=45) ---
    kw = dict(bpm=(82,98), feel="straight-16", tags=["hip-hop","boom-bap","bass"],
              prov=BB, harmonic="minor")
    c = bass(g,"Walking Roots","core",
        [(0,0,0.9,'a'),(6,-2,0.5,'n'),(10,-4,0.5,'n'),(12,-5,0.9,'n')],
        desc="Upright-style walk following the kick: root, flat-7, flat-6, down to the fifth. Short, plucked notes.",
        energy="mid", density="core", **kw)
    s = bass(g,"Root Pulse","sparse",
        [(0,0,1.5,'a'),(8,-5,1.5,'n')],
        desc="Sparse two-note root pulse anchoring the pocket.",
        energy="low", density="sparse", **kw)
    v = bass(g,"Fifth Walk","dense",
        [(0,0,0.5,'a'),(3,7,0.5,'n'),(6,-2,0.5,'n'),(8,0,0.5,'n'),(10,-4,0.5,'n'),(12,-5,0.5,'n'),(14,7,0.5,'s')],
        desc="Busier walking line adding the fifth and passing tones under the groove.",
        energy="mid", density="dense", **kw)
    family("boom-bap-bass-walk","Upright Walk","bass",g,
           [("core",c),("sparse",s),("dense",v)])

    # Boom-bap chord keys (Rhodes Min7 Stabs, Jazz ii-V-i) moved to role "chords"
    # (see packs_chords.py). Only mono/riff synth lines remain in this pack.

    # ================= TRAP =================
    g = "trap"
    kw = dict(bpm=(130,150), feel="half-time", tags=["trap","hats"], prov=TR)
    c = drums(g,"Trap Core","core",
        {"BD":R(16,{0:'X',6:'x',10:'x'}),
         "SD":R(16,{8:'X'}),
         "CH":R(16,{i:('X' if i in(0,4,8,12) else 'x') for i in range(16)}),
         "OH":R(16,{10:'x'})},
        ratch={"CH":{7:2,14:3,15:4}}, prob={"OH":{10:0.3},"CH":{3:0.85,5:0.85,11:0.85,13:0.85}},
        desc="Half-time trap: syncopated kick, snare on beat 3, sixteenth hats with sub-step ratchet rolls toward the bar end.",
        energy="mid", density="core", **kw)
    s = drums(g,"Sparse 808","sparse",
        {"BD":R(16,{0:'X',10:'x'}),"SD":R(16,{8:'X'}),
         "CH":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        ratch={"CH":{15:3}},
        desc="Spacious half-time skeleton: two kicks, snare on 3, quarter hats with one closing triplet-ratchet.",
        energy="low", density="sparse", **kw)
    d = drums(g,"Roll Up","dense",
        {"BD":R(16,{0:'X',6:'x',10:'x',12:'x'}),"SD":R(16,{8:'X'}),
         "CH":R(16,{i:('X' if i in(0,4,8,12) else 'x') for i in range(16)}),"OH":R(16,{10:'x'})},
        ratch={"CH":{3:2,7:2,11:3,13:2,15:6}},
        desc="Dense rolling hats: escalating ratchet counts across the bar for the classic hat roll-up.",
        energy="high", density="dense", **kw)
    f = drums(g,"Hat Roll Fill","fill",
        {"BD":R(16,{0:'X'}),"SD":R(16,{8:'X'}),
         "CH":R(16,{0:'x',4:'x',8:'x',12:'x',13:'x',14:'x',15:'x'}),"CC":R(16,{0:'X'})},
        ratch={"CH":{12:3,13:4,14:6,15:8}},
        desc="Accelerating hi-hat roll fill (ratchet 3-4-6-8) into a crash on the one.",
        energy="high", density="dense", **kw)
    b = drums(g,"Drop-Out","breakdown",
        {"BD":R(16,{0:'X'}),"CH":R(16,{8:'x'})},
        ratch={"CH":{8:3}},
        desc="Breakdown: a single kick and one ratcheted hat, near-silence for tension.",
        energy="low", density="sparse", **kw)
    family("trap-drums-hats","Trap Hats","drums",g,
           [("core",c),("sparse",s),("dense",d),("fill",f),("breakdown",b)])

    kw = dict(bpm=(130,150), feel="half-time", tags=["trap","808","bass"], prov=TR, harmonic="minor")
    c = bass(g,"808 Sub","core",
        [(0,0,3.0,'a'),(6,0,1.0,'n'),(10,3,2.5,'n','slide')],
        desc="808-style sub locked to the kick, with one pitch-glide up to the flat-third.",
        energy="mid", density="core", **kw)
    s = bass(g,"Minimal Sub","sparse",
        [(0,0,4.0,'a'),(8,-2,4.0,'n','slide')],
        desc="Two long sub notes with a slow glide — maximum space.",
        energy="low", density="sparse", **kw)
    v = bass(g,"Bounce 808","dense",
        [(0,0,1.5,'a'),(4,0,0.5,'n'),(6,3,0.5,'n'),(10,-2,1.0,'n','slide'),(13,0,1.0,'n')],
        desc="Bouncier 808 with more note movement and a mid-bar glide.",
        energy="mid", density="dense", **kw)
    family("trap-bass-808","808 Glide","bass",g,[("core",c),("sparse",s),("dense",v)])

    # Minor Bell Stabs (chord triad) moved to role "chords" (packs_chords.py).
    # The monophonic Phrygian Motif stays under synth as the family core.
    kw = dict(bpm=(130,150), feel="half-time", tags=["trap","dark","keys"], prov=TR, harmonic="minor")
    v = synth(g,"Phrygian Motif","core",
        [(0,0,0.5,'a'),(2,1,0.5,'n'),(4,3,0.5,'n'),(6,0,0.5,'n'),(8,-2,1.0,'n'),(12,1,0.5,'n')],
        desc="Single-line Phrygian motif leaning on the flat-second for menace.",
        energy="mid", density="core", **kw)
    family("trap-synth-dark","Dark Trap Lead","synth",g,[("core",v)])


# ============================ PACK 6: MODERN CLUB ============================
def pack_club():
    TH = prov("modern-club", ["Tech-house production guides (Beatportal, Samplesound)",
                              "Roland TR-909 four-on-floor heritage","Magenta Groove MIDI Dataset"])
    MT = prov("modern-club", ["House vs Techno reference (Splice)","Melodic-techno production references",
                              "Roland TR-909 heritage"])
    HT = prov("modern-club", ["Techno BPM guide (ZIPDJ)","Hard-techno production references",
                              "Roland TR-909 heritage"])
    FW = prov("modern-club", ["Footwork/Teklife references (Wikipedia, soundwitches)",
                              "Magenta Groove MIDI Dataset","Roland TR-808/909 heritage"])

    # ---- tech-house ----
    g="tech-house"
    kw=dict(bpm=(124,128), feel="straight", tags=["tech-house","rolling"], prov=TH)
    c=drums(g,"Rolling Core","core",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),
         "CH":R(16,{i:'x' for i in range(16)}),"RS":R(16,{4:'x',12:'x'}),"CB":R(16,{3:'x',10:'x'})},
        prob={"CH":{1:0.7,3:0.7,5:0.7,7:0.7,9:0.7,11:0.7,13:0.7,15:0.7}},
        desc="Four-on-the-floor with offbeat open hats, rolling sixteenth closed hats and a syncopated rim/cowbell tick.",
        energy="mid", density="core", **kw)
    s=drums(g,"Offbeat Skip","sparse",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),
         "CH":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        desc="Stripped skip groove: four-on-floor, offbeat open hats and quarter closed hats.",
        energy="low", density="sparse", **kw)
    d=drums(g,"Percussion Tool","dense",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),
         "CH":R(16,{i:'x' for i in range(16)}),"CB":R(16,{1:'x',3:'x',7:'x',10:'x',13:'x'}),
         "RC":R(16,{2:'x',6:'x',10:'x',14:'x'}),"RS":R(16,{4:'x',12:'x'})},
        desc="Busy percussion tool layering cowbell syncopation and a ride shimmer over the roll.",
        energy="high", density="dense", **kw)
    f=drums(g,"Tech Fill","fill",
        {"BD":R(16,{0:'X',4:'X',8:'X'}),"RS":R(16,{12:'x',13:'x',14:'x',15:'x'}),"CC":R(16,{0:'X'})},
        ratch={"RS":{13:2,14:3,15:4}},
        desc="Rim-shot ratchet roll across the last beat into a crash on the one.",
        energy="high", density="dense", **kw)
    family("tech-house-drums-roller","Tech-House Roller","drums",g,
           [("core",c),("sparse",s),("dense",d),("fill",f)])

    kw=dict(bpm=(124,128), feel="straight", tags=["tech-house","rolling","bass"], prov=TH, harmonic="minor")
    c=bass(g,"Rolling Offbeat","core",
        [(2,0,0.4,'n'),(6,0,0.4,'n'),(10,-2,0.4,'n'),(14,0,0.4,'n')],
        desc="Classic rolling offbeat bass: short notes ducking between the kicks.",
        energy="mid", density="core", **kw)
    v=bass(g,"Octave Roll","dense",
        [(2,0,0.4,'n'),(3,12,0.4,'s'),(6,0,0.4,'n'),(7,12,0.4,'s'),(10,-2,0.4,'n'),(14,0,0.4,'n'),(15,12,0.4,'s')],
        desc="Sixteenth octave-bounce roller for driving energy.",
        energy="high", density="dense", **kw)
    family("tech-house-bass-roll","Offbeat Roller","bass",g,[("core",c),("dense",v)])

    # Minimal Min9 Stab (chord) moved to role "chords" (packs_chords.py).

    # ---- melodic-techno ----
    g="melodic-techno"
    kw=dict(bpm=(120,126), feel="straight", tags=["melodic-techno","deep"], prov=MT)
    c=drums(g,"Melodic Pulse","core",
        {"BD":R(16,{0:'X',4:'X',8:'X',12:'X'}),"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),
         "CH":R(16,{i:'x' for i in range(16)}),"RS":R(16,{6:'o',14:'o'})},
        prob={"CH":{1:0.5,3:0.5,5:0.5,7:0.5,9:0.5,11:0.5,13:0.5,15:0.5}},
        desc="Deep soft four-on-floor with restrained offbeat open hats; hat presence driven by probability.",
        energy="mid", density="core", **kw)
    b=drums(g,"Deep Pad Bed","breakdown",
        {"OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),"RC":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        desc="Kick-less breakdown bed: offbeat open hats and a ride shimmer under the pad.",
        energy="low", density="sparse", **kw)
    family("melodic-techno-drums-pulse","Melodic Pulse","drums",g,[("core",c),("breakdown",b)])

    kw=dict(bpm=(120,126), feel="straight", tags=["melodic-techno","arp","hypnotic"], prov=MT, harmonic="minor")
    c=synth(g,"Minor Arp","core",
        [(0,0,0.5,'a'),(2,7,0.5,'n'),(4,12,0.5,'n'),(6,7,0.5,'n'),(8,10,0.5,'n'),(10,7,0.5,'n'),(12,3,0.5,'n'),(14,7,0.5,'n')],
        desc="Hypnotic eighth-note minor arpeggio outlining the chord over the bar.",
        energy="mid", density="core", **kw)
    d=synth(g,"Arp Builder","dense",
        [(i,[0,7,12,15,19][i%5],0.25,'n') for i in range(16)],
        desc="Sixteenth-note arpeggio builder climbing the minor chord tones with a filter opening.",
        energy="high", density="dense", cc={0:[(74,30)],8:[(74,80)],15:[(74,120)]}, **kw)
    # Melodic Breakdown (sustained pad chords) moved to role "chords".
    family("melodic-techno-synth-arp","Melodic Arp","synth",g,[("core",c),("dense",d)])

    kw=dict(bpm=(120,126), feel="straight", tags=["melodic-techno","sub","bass"], prov=MT, harmonic="minor")
    c=bass(g,"Sustained Sub","core",
        [(0,0,4.0,'a'),(8,-2,4.0,'n','slide')],
        desc="Two sustained sub roots with a slow glide and a filter build.",
        energy="mid", density="core", cc={0:[(74,40)],8:[(74,90)]}, **kw)
    family("melodic-techno-bass-sub","Sustained Sub","bass",g,[("core",c)])

    # ---- hard-techno ----
    g="hard-techno"
    kw=dict(bpm=(140,155), feel="straight", tags=["hard-techno","driving"], prov=HT)
    c=drums(g,"Hard Groove","core",
        {"BD":R(16,{0:'^',4:'^',8:'^',12:'^',2:'o',6:'o',10:'o',14:'o'}),
         "OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),"CH":R(16,{i:'x' for i in range(16)})},
        prob={"BD":{2:0.5,6:0.5,10:0.5,14:0.5}},
        desc="Driving four-on-floor with probabilistic offbeat ghost kicks for the hard 'rolling' groove.",
        energy="high", density="core", **kw)
    d=drums(g,"Peak Pounder","peak",
        {"BD":R(16,{i:('^' if i%4==0 else 'o') for i in range(16)}),
         "OH":R(16,{2:'x',6:'x',10:'x',14:'x'}),"CH":R(16,{i:'x' for i in range(16)})},
        ratch={"CH":{15:4}},
        desc="Relentless sixteenth kick rumble under offbeat hats — peak-time weapon.",
        energy="high", density="dense", **kw)
    f=drums(g,"Tribal Tom Fill","fill",
        {"BD":R(16,{0:'^',4:'^',8:'^'}),"MT":R(16,{9:'x',11:'x',13:'x'}),"HT":R(16,{10:'x',12:'x',14:'x'}),"CC":R(16,{0:'X'})},
        ratch={"HT":{14:3}},
        desc="Two-tom tribal cascade with a ratcheted fill into a crash.",
        energy="high", density="dense", **kw)
    family("hard-techno-drums-groove","Hard Groove","drums",g,[("core",c),("dense",d),("fill",f)])

    kw=dict(bpm=(140,155), feel="straight", tags=["hard-techno","acid","bass"], prov=HT, harmonic="phrygian")
    c=bass(g,"Rolling Rumble","core",
        [(i,0,0.25,'n') for i in range(16)],
        desc="Sixteenth-note single-note rumble locked under the kick.",
        energy="high", density="dense", **kw)
    v=bass(g,"Acid Line","dense",
        [(0,0,0.25,'a'),(2,0,0.25,'n'),(3,1,0.25,'s'),(6,12,0.25,'n'),(8,0,0.25,'n'),(10,3,0.25,'s'),(11,1,0.25,'n'),(14,0,0.25,'n')],
        desc="Phrygian acid line with slides and accents for hypnotic tension.",
        energy="high", density="dense", **kw)
    family("hard-techno-bass-acid","Rolling Rumble","bass",g,[("core",c),("dense",v)])

    # ---- footwork ----
    g="footwork"
    kw=dict(bpm=(158,162), feel="straight", tags=["footwork","juke","triplet"], prov=FW)
    c=drums(g,"Footwork Skitter","core",
        {"BD":R(16,{0:'X',3:'x',6:'x',10:'x',13:'x'}),
         "SD":R(16,{8:'X'}),"RS":R(16,{2:'x',12:'x'}),
         "CH":R(16,{i:'x' for i in range(16)})},
        ratch={"SD":{8:3},"CH":{5:3,13:3}},
        desc="Syncopated footwork kick with a triplet-ratchet snare backbeat and skittering triplet-ratchet hats.",
        energy="high", density="core", **kw)
    s=drums(g,"Sparse Bounce","sparse",
        {"BD":R(16,{0:'X',6:'x',12:'x'}),"RS":R(16,{8:'x'}),"CH":R(16,{0:'x',4:'x',8:'x',12:'x'})},
        ratch={"CH":{15:3}},
        desc="Sparser juke bounce: three kicks, a rim backbeat and quarter hats.",
        energy="mid", density="sparse", **kw)
    d=drums(g,"Triplet Juke","dense",
        {"BD":R(16,{0:'X',3:'x',6:'x',8:'x',10:'x',13:'x'}),"SD":R(16,{4:'x',8:'X',12:'x'}),
         "CH":R(16,{i:'x' for i in range(16)})},
        ratch={"SD":{4:3,8:3,12:3},"CH":{2:3,6:3,10:3,14:3}},
        desc="Dense triplet-ratchet snare and hat rolls throughout — full juke intensity.",
        energy="high", density="dense", **kw)
    family("footwork-drums-skitter","Footwork Skitter","drums",g,
           [("core",c),("sparse",s),("dense",d)])

    kw=dict(bpm=(158,162), feel="straight", tags=["footwork","juke","sub","bass"], prov=FW, harmonic="minor")
    c=bass(g,"Juke Sub","core",
        [(0,0,1.0,'a'),(6,0,0.5,'n'),(8,-5,1.0,'n','slide'),(13,0,0.5,'n')],
        desc="Syncopated 808-style sub tracking the footwork kick with one glide.",
        energy="high", density="core", **kw)
    family("footwork-bass-sub","Juke Sub","bass",g,[("core",c)])
