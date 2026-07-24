// Central reactive state. Holds the latest backend snapshot plus library/set
// listings. All UI reads from `state`; all mutations go out through `send`.

import {
  addChainEntry as bridgeAddChainEntry,
  auditionPattern,
  crateAdd as bridgeCrateAdd,
  crateCreate as bridgeCrateCreate,
  crateDelete as bridgeCrateDelete,
  crateLaunch as bridgeCrateLaunch,
  crateMoveEntry as bridgeCrateMoveEntry,
  crateRemoveEntry as bridgeCrateRemoveEntry,
  crateRename as bridgeCrateRename,
  applyChordProgression as bridgeApplyChords,
  dispatch as bridgeDispatch,
  getAppVersion,
  getLibrary,
  getScales,
  getSetList,
  getSnapshot,
  loadLibraryPattern,
  getUserPatterns,
  onSnapshot,
  onTransport,
  placeNote as bridgePlaceNote,
  stopAudition,
  toggleFavorite,
} from "./bridge";
import type {
  GuiCommand,
  LibraryData,
  SetEntry,
  Snapshot,
  UserPatternEntry,
} from "./types";

interface AppState {
  snap: Snapshot | null;
  library: LibraryData | null;
  sets: SetEntry[];
  userPatterns: UserPatternEntry[];
  /** Scale names in backend order; index is what a SetScale command expects. */
  scales: string[];
  version: string;
  ready: boolean;
  error: string | null;
}

export const app = $state<AppState>({
  snap: null,
  library: null,
  sets: [],
  userPatterns: [],
  scales: [],
  version: "",
  ready: false,
  error: null,
});

/// Fetch the initial snapshot and subscribe to backend events.
export async function init(): Promise<void> {
  try {
    app.snap = await getSnapshot();
    app.ready = true;
    await onSnapshot((s) => {
      app.snap = s;
    });
    await onTransport((t) => {
      if (app.snap) app.snap.transport = t;
    });
    app.library = await getLibrary();
    app.scales = await getScales();
    app.sets = await getSetList();
    app.userPatterns = await getUserPatterns();
    try {
      app.version = await getAppVersion();
    } catch {
      // Version is cosmetic; ignore if the app plugin isn't reachable.
    }
  } catch (e) {
    app.error = String(e);
  }
}

export async function send(cmd: GuiCommand): Promise<void> {
  try {
    app.snap = await bridgeDispatch(cmd);
  } catch (e) {
    app.error = String(e);
  }
}

export async function loadPattern(
  role: string,
  genre: string,
  name: string,
): Promise<void> {
  try {
    app.snap = await loadLibraryPattern(role, genre, name);
  } catch (e) {
    app.error = String(e);
  }
}

export async function placeNote(
  lane: number,
  col: number,
  pitch: number,
): Promise<void> {
  try {
    app.snap = await bridgePlaceNote(lane, col, pitch);
  } catch (e) {
    app.error = String(e);
  }
}

/// Apply a typed chord progression to `lane`. Returns a parse-error string when
/// the text was invalid (the modal keeps it open and shows the message), else null.
export async function applyChordProgression(
  lane: number,
  text: string,
): Promise<string | null> {
  try {
    const [err, snap] = await bridgeApplyChords(lane, text);
    app.snap = snap;
    return err;
  } catch (e) {
    app.error = String(e);
    return String(e);
  }
}

export async function audition(
  role: string,
  genre: string,
  name: string,
): Promise<void> {
  try {
    app.snap = await auditionPattern(role, genre, name);
  } catch (e) {
    app.error = String(e);
  }
}

export async function endAudition(): Promise<void> {
  try {
    app.snap = await stopAudition();
  } catch (e) {
    app.error = String(e);
  }
}

export async function favorite(
  role: string,
  genre: string,
  name: string,
): Promise<void> {
  try {
    app.library = await toggleFavorite(role, genre, name);
  } catch (e) {
    app.error = String(e);
  }
}

/// Run a user-pattern-store command, then refresh the library (which carries the
/// injected "User" genre) and the management list.
export async function userPatternCmd(cmd: GuiCommand): Promise<void> {
  try {
    app.snap = await bridgeDispatch(cmd);
    app.library = await getLibrary();
    app.userPatterns = await getUserPatterns();
  } catch (e) {
    app.error = String(e);
  }
}

export async function addChainEntry(chain: number, scene: number): Promise<void> {
  try {
    app.snap = await bridgeAddChainEntry(chain, scene);
  } catch (e) {
    app.error = String(e);
  }
}

async function applySnap(p: Promise<Snapshot>): Promise<void> {
  try {
    app.snap = await p;
  } catch (e) {
    app.error = String(e);
  }
}
export const crateCreate = (name: string) => applySnap(bridgeCrateCreate(name));
export const crateRename = (i: number, name: string) => applySnap(bridgeCrateRename(i, name));
export const crateDelete = (i: number) => applySnap(bridgeCrateDelete(i));
export const crateAdd = (c: number, role: string, genre: string, name: string) =>
  applySnap(bridgeCrateAdd(c, role, genre, name));
export const crateRemoveEntry = (c: number, e: number) => applySnap(bridgeCrateRemoveEntry(c, e));
export const crateMoveEntry = (c: number, from: number, to: number) =>
  applySnap(bridgeCrateMoveEntry(c, from, to));
export const crateLaunch = (c: number, e: number) => applySnap(bridgeCrateLaunch(c, e));

export async function refreshSets(): Promise<void> {
  try {
    app.sets = await getSetList();
  } catch (e) {
    app.error = String(e);
  }
}
