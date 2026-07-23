// Typed wrapper over the Tauri command/event bridge. This is the ONLY module
// that talks to the backend; everything else goes through these functions.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  GuiCommand,
  LibraryData,
  SetEntry,
  Snapshot,
  Transport,
} from "./types";

export const getSnapshot = (): Promise<Snapshot> => invoke("gui_snapshot");

export const dispatch = (cmd: GuiCommand): Promise<Snapshot> =>
  invoke("gui_dispatch", { cmd });

export const getLibrary = (): Promise<LibraryData> => invoke("gui_library");

export const loadLibraryPattern = (
  role: string,
  genre: string,
  name: string,
): Promise<Snapshot> => invoke("gui_load_pattern", { role, genre, name });

export const getSetList = (): Promise<SetEntry[]> => invoke("gui_set_list");

export const onSnapshot = (cb: (s: Snapshot) => void): Promise<UnlistenFn> =>
  listen<Snapshot>("snapshot", (e) => cb(e.payload));

export const onTransport = (cb: (t: Transport) => void): Promise<UnlistenFn> =>
  listen<Transport>("transport", (e) => cb(e.payload));
