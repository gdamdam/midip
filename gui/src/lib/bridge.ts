// Typed wrapper over the Tauri command/event bridge. This is the ONLY module
// that talks to the backend; everything else goes through these functions.

import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  GuiCommand,
  LibQuery,
  LibRecord,
  LibraryData,
  SetEntry,
  Snapshot,
  Transport,
  UserPatternEntry,
} from "./types";

export const getAppVersion = (): Promise<string> => getVersion();

export const getSnapshot = (): Promise<Snapshot> => invoke("gui_snapshot");

export const dispatch = (cmd: GuiCommand): Promise<Snapshot> =>
  invoke("gui_dispatch", { cmd });

export const getLibrary = (): Promise<LibraryData> => invoke("gui_library");

// Scale names in Scale::all() order; the index is what a SetScale command expects.
export const getScales = (): Promise<string[]> => invoke("gui_scales");

export const loadLibraryPattern = (
  role: string,
  genre: string,
  name: string,
): Promise<Snapshot> => invoke("gui_load_pattern", { role, genre, name });

export const placeNote = (
  lane: number,
  col: number,
  pitch: number,
): Promise<Snapshot> => invoke("gui_place_note", { lane, col, pitch });

// Build a chord progression from typed chord names onto `lane`. Returns the
// parse error (or null) plus the fresh snapshot.
export const applyChordProgression = (
  lane: number,
  text: string,
): Promise<[string | null, Snapshot]> =>
  invoke("gui_apply_chord_progression", { lane, text });

export const auditionPattern = (
  role: string,
  genre: string,
  name: string,
): Promise<Snapshot> => invoke("gui_audition", { role, genre, name });

export const stopAudition = (): Promise<Snapshot> => invoke("gui_stop_audition");

// Phase 8: shared library filter engine. Returns already-filtered, sorted records.
export const libraryQuery = (query: LibQuery): Promise<LibRecord[]> =>
  invoke("gui_library_query", { query });

export const toggleFavorite = (
  role: string,
  genre: string,
  name: string,
): Promise<LibraryData> => invoke("gui_toggle_favorite", { role, genre, name });

export const addChainEntry = (chain: number, scene: number): Promise<Snapshot> =>
  invoke("gui_add_chain_entry", { chain, scene });

// Crates. Keys are snake_case to match the Rust command params exactly.
export const crateCreate = (name: string): Promise<Snapshot> =>
  invoke("gui_crate_create", { name });
export const crateRename = (index: number, name: string): Promise<Snapshot> =>
  invoke("gui_crate_rename", { index, name });
export const crateDelete = (index: number): Promise<Snapshot> =>
  invoke("gui_crate_delete", { index });
export const crateAdd = (
  crateIdx: number,
  role: string,
  genre: string,
  name: string,
): Promise<Snapshot> => invoke("gui_crate_add", { crate_idx: crateIdx, role, genre, name });
export const crateRemoveEntry = (crateIdx: number, entry: number): Promise<Snapshot> =>
  invoke("gui_crate_remove_entry", { crate_idx: crateIdx, entry });
export const crateMoveEntry = (crateIdx: number, from: number, to: number): Promise<Snapshot> =>
  invoke("gui_crate_move_entry", { crate_idx: crateIdx, from, to });
export const crateLaunch = (crateIdx: number, entry: number): Promise<Snapshot> =>
  invoke("gui_crate_launch", { crate_idx: crateIdx, entry });

export const getSetList = (): Promise<SetEntry[]> => invoke("gui_set_list");

export const getOutputPorts = (): Promise<string[]> => invoke("gui_output_ports");

export const getInputPorts = (): Promise<string[]> => invoke("gui_input_ports");

export const getUserPatterns = (): Promise<UserPatternEntry[]> =>
  invoke("gui_user_patterns");

export const onSnapshot = (cb: (s: Snapshot) => void): Promise<UnlistenFn> =>
  listen<Snapshot>("snapshot", (e) => cb(e.payload));

export const onTransport = (cb: (t: Transport) => void): Promise<UnlistenFn> =>
  listen<Transport>("transport", (e) => cb(e.payload));
