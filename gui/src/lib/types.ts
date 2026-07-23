// TypeScript mirror of the Rust GUI DTOs (gui/src-tauri/src/dto.rs) and the
// GuiCommand surface (command.rs). Kept in lockstep by hand — the shapes are
// small and stable.

export type TrigCond =
  | { type: "Always" }
  | { type: "Ratio"; x: number; y: number }
  | { type: "Fill" }
  | { type: "NotFill" }
  | { type: "First" }
  | { type: "NotFirst" };

export interface DrumHit {
  note: number;
  vel: number; // 1..=127
  prob: number; // 0..1
  ratchet: number; // 1..8
  micro: number; // ticks
  cond: TrigCond;
}

export interface MelodicNote {
  semi: number;
  pitch: number; // resolved MIDI pitch
  vel: number; // multiplier 0.5..1.3
  slide: boolean;
  len: number; // steps
  prob: number;
  ratchet: number;
  micro: number;
  cond: TrigCond;
}

export interface CcLock {
  cc: number;
  val: number;
}

export interface Position {
  bar: number;
  beat: number;
  sixteenth: number;
}

export interface ClockIn {
  locked: boolean;
  tempo: number;
  port: string;
}

export interface Transport {
  playing: boolean;
  armed: boolean;
  bpm: number;
  set_bpm: number;
  swing: number; // 0..1
  link_enabled: boolean;
  link_peers: number;
  position: Position;
  playhead: number;
  dirty: boolean;
  mirror: boolean;
  clock_in: ClockIn | null;
  set_name: string;
}

export interface Lane {
  index: number;
  kind: "drums" | "melodic";
  role: string;
  label: string;
  pattern_name: string;
  connected: boolean;
  device: string;
  channel: number; // 1-based
  mute: boolean;
  solo: boolean;
  length: number;
  queued: string | null;
  transpose: number;
  octave: number;
  focused: boolean;
  route_port: string;
  route_default: boolean;
  clock_out: boolean;
  swing: number | null;
  clock_div: number | null;
}

export interface Voice {
  note: number;
  label: string;
  muted: boolean;
}

export interface Pattern {
  kind: "drums" | "melodic";
  name: string;
  length: number;
  voices: Voice[];
  drum_steps: DrumHit[][];
  melodic_steps: MelodicNote[][];
  scale: string;
  root: number;
  octave: number;
  transpose: number;
  cc: CcLock[][];
  muted_voices: number[];
}

export interface Selection {
  row: number;
  col: number;
}

export interface Inspector {
  kind: "drums" | "melodic";
  present: boolean;
  velocity: number | null;
  vel_mult: number | null;
  probability: number | null;
  ratchet: number | null;
  length: number | null;
  slide: boolean | null;
  micro: number | null;
  cond: TrigCond | null;
  pitch: number | null;
  cc: CcLock[];
}

export interface SceneItem {
  index: number;
  name: string;
}
export interface ChainEntryItem {
  scene: string;
  repeats: number;
  bars: number;
}
export interface ChainItem {
  index: number;
  name: string;
  looped: boolean;
  entries: ChainEntryItem[];
  current_entry: number | null;
}
export interface Song {
  scenes: SceneItem[];
  chains: ChainItem[];
  playing_chain: number | null;
}

export interface Gen {
  active: boolean;
  mode: "generate" | "vary" | "arp";
  density: number;
  range: number;
  mutate: number;
  arp_chord: "power" | "triad" | "seventh" | "octaves";
  arp_octaves: number;
  arp_shape: "up" | "down" | "updown" | "random";
  arp_gate: number;
  arp_vel_var: number;
  melodic: boolean;
}

export interface Snapshot {
  transport: Transport;
  lanes: Lane[];
  focused_lane: number;
  focused_pattern: Pattern;
  selection: Selection;
  inspector: Inspector;
  song: Song;
  gen: Gen;
  status: string;
}

// --- Library ---

export interface LibPattern {
  name: string;
  length: number;
  kind: "drums" | "melodic";
  favorite: boolean;
}
export interface LibGenre {
  name: string;
  patterns: LibPattern[];
}
export interface LibRole {
  role: string;
  genres: LibGenre[];
}
export interface LibraryData {
  roles: LibRole[];
}

export interface SetEntry {
  name: string;
  path: string;
}

export interface UserPatternEntry {
  name: string;
  path: string;
  kind: "drums" | "melodic";
  length: number;
}

// --- Commands (adjacently-tagged: {type, args?}) ---

export type GuiCommand =
  | { type: "togglePlay" }
  | { type: "setBpm"; args: number }
  | { type: "adjustBpm"; args: number }
  | { type: "tap" }
  | { type: "adjustSwing"; args: number }
  | { type: "toggleLink" }
  | { type: "panic" }
  | { type: "toggleMirror" }
  | { type: "focusLane"; args: number }
  | { type: "toggleMute"; args: number }
  | { type: "toggleSolo"; args: number }
  | { type: "cancelQueue"; args: number }
  | { type: "toggleVoiceMute"; args: { lane: number; row: number } }
  | { type: "adjustPatternLen"; args: { lane: number; delta: number } }
  | { type: "clearPattern"; args: number }
  | { type: "doubleLength"; args: number }
  | { type: "selectStep"; args: { lane: number; row: number; col: number } }
  | { type: "toggleStep"; args: { lane: number; row: number; col: number } }
  | { type: "clearStep"; args: { lane: number; row: number; col: number } }
  | { type: "setVelBucket"; args: { lane: number; row: number; col: number; bucket: number } }
  | { type: "adjustVel"; args: { lane: number; row: number; col: number; delta: number } }
  | { type: "adjustProb"; args: { lane: number; row: number; col: number; delta: number } }
  | { type: "adjustRatchet"; args: { lane: number; row: number; col: number; delta: number } }
  | { type: "adjustMicro"; args: { lane: number; row: number; col: number; delta: number } }
  | { type: "cycleCond"; args: { lane: number; row: number; col: number } }
  | { type: "adjustLen"; args: { lane: number; row: number; col: number; delta: number } }
  | { type: "toggleSlide"; args: { lane: number; row: number; col: number } }
  | { type: "noteUp"; args: { lane: number; col: number } }
  | { type: "noteDown"; args: { lane: number; col: number } }
  | { type: "copyStep"; args: { lane: number; row: number; col: number } }
  | { type: "cutStep"; args: { lane: number; row: number; col: number } }
  | { type: "pasteStep"; args: { lane: number; row: number; col: number } }
  | { type: "ccAdd"; args: { lane: number; row: number; col: number } }
  | { type: "ccRemove"; args: { lane: number; row: number; col: number } }
  | { type: "adjustCcVal"; args: { lane: number; row: number; col: number; delta: number } }
  | { type: "adjustLaneSwing"; args: { lane: number; delta: number } }
  | { type: "clearLaneSwing"; args: number }
  | { type: "cycleClockDiv"; args: number }
  | { type: "euclid"; args: { lane: number; row: number; dp: number; dr: number } }
  | { type: "rotateRight"; args: number }
  | { type: "rotateLeft"; args: number }
  | { type: "conformToScale"; args: number }
  | { type: "toggleFill"; args: number }
  | { type: "commitTransform"; args: number }
  | { type: "setClockIn"; args: number }
  | { type: "renameSet"; args: string }
  | { type: "duplicateSet" }
  | { type: "deleteSet"; args: string }
  | { type: "cycleRoutePort"; args: { lane: number; delta: number } }
  | { type: "adjustRouteChannel"; args: { lane: number; delta: number } }
  | { type: "toggleClockOut"; args: number }
  | { type: "cycleScale"; args: { lane: number; delta: number } }
  | { type: "adjustRoot"; args: { lane: number; delta: number } }
  | { type: "adjustOctave"; args: { lane: number; delta: number } }
  | { type: "recallScene"; args: number }
  | { type: "captureScene" }
  | { type: "playChain"; args: number }
  | { type: "stopChain" }
  | { type: "openGenerative" }
  | { type: "genSetMode"; args: string }
  | { type: "genAdjust"; args: { field: string; delta: number } }
  | { type: "genReroll" }
  | { type: "genCommit" }
  | { type: "genCancel" }
  | { type: "undo" }
  | { type: "redo" }
  | { type: "save" }
  | { type: "saveSetAs"; args: string }
  | { type: "newSet" }
  | { type: "loadSet"; args: string }
  | { type: "loadUserPattern"; args: string }
  | { type: "saveLanePattern"; args: string }
  | { type: "renameUserPattern"; args: { path: string; name: string } }
  | { type: "duplicateUserPattern"; args: string }
  | { type: "deleteUserPattern"; args: string };
