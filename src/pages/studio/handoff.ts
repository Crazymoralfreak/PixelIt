// A request handed from the history or from a result back to the form.

import type { GeneratedRecord } from "../../types";

export type StudioHandoff =
  | { token: number; kind: "repeat"; record: GeneratedRecord }
  | { token: number; kind: "seed"; seed: number }
  | { token: number; kind: "edit"; path: string }
  | { token: number; kind: "similar"; record: GeneratedRecord; path: string }
  | { token: number; kind: "variants"; record: GeneratedRecord }
  | { token: number; kind: "drop"; path: string };
