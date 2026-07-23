// Central reactive state. Holds the latest backend snapshot plus library/set
// listings. All UI reads from `state`; all mutations go out through `send`.

import {
  auditionPattern,
  dispatch as bridgeDispatch,
  getLibrary,
  getSetList,
  getSnapshot,
  loadLibraryPattern,
  onSnapshot,
  onTransport,
  stopAudition,
  toggleFavorite,
} from "./bridge";
import type { GuiCommand, LibraryData, SetEntry, Snapshot } from "./types";

interface AppState {
  snap: Snapshot | null;
  library: LibraryData | null;
  sets: SetEntry[];
  ready: boolean;
  error: string | null;
}

export const app = $state<AppState>({
  snap: null,
  library: null,
  sets: [],
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
    app.sets = await getSetList();
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

export async function refreshSets(): Promise<void> {
  try {
    app.sets = await getSetList();
  } catch (e) {
    app.error = String(e);
  }
}
