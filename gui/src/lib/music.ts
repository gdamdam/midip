// Small presentation helpers. Naming lives on the frontend; the engine sends
// resolved MIDI pitch numbers.

import type { TrigCond } from "./types";

const NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

export function midiName(pitch: number): string {
  const n = ((pitch % 12) + 12) % 12;
  const octave = Math.floor(pitch / 12) - 1;
  return `${NAMES[n]}${octave}`;
}

export function condLabel(c: TrigCond | null | undefined): string {
  if (!c) return "—";
  switch (c.type) {
    case "Always":
      return "Always";
    case "Ratio":
      return `${c.x}:${c.y}`;
    case "Fill":
      return "Fill";
    case "NotFill":
      return "!Fill";
    case "First":
      return "1st";
    case "NotFirst":
      return "!1st";
  }
}

/** Ember accent for a lane by musical role (hardware-neutral). */
export function roleColor(role: string): string {
  switch (role) {
    case "drums":
      return "var(--ember)";
    case "bass":
      return "var(--pink)";
    case "chords":
      return "var(--green)";
    case "synth":
      return "var(--aqua)";
    default:
      return "var(--fg-dim)";
  }
}
