import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export type ViewName = "conversation" | "sources" | "settings";

export interface AppStatus {
  sources: number;
  documents: number;
  concepts: number;
  values: number;
}

export interface SourceSummary {
  id: number;
  path: string;
  document_count: number;
  indexed_at: string | null;
}

export interface IndexReport {
  source_id: number;
  discovered: number;
  indexed: number;
  modified: number;
  skipped: number;
  ocr_pending: number;
  values: number;
  warnings: string[];
  elapsed_ms: number;
}

export interface Evidence {
  id: string;
  document_id: number;
  path: string;
  origin: string;
  location: string;
  excerpt: string;
  normalized_value: string | null;
  value: string | null;
  matched: string | null;
  field: string | null;
  match_kind: "exacta" | "canónica" | "campo" | "texto" | "prefijo" | "contiene" | "cálculo";
  reliable: boolean;
  confidence: number | null;
}

export interface DateConstraint {
  concept: string;
  from: string;
  to: string;
}

export interface ToolFilter {
  concept: string;
  equals: string;
}

/** Alcance exacto que el motor consultó para producir la respuesta. */
export interface AnswerScope {
  filters: ToolFilter[];
  origin: string | null;
  concept: string | null;
  group_by: string | null;
  date: DateConstraint | null;
  currency: string | null;
  document_count: number | null;
  value_count: number | null;
  inherited: boolean;
}

/** Pregunta que Omega devuelve cuando no puede saber a qué se refiere. */
export interface Clarification {
  question: string;
  options: string[];
  reason: string;
}

/** Un documento que Omega abrió y leyó completo para redactar el resumen. */
export interface ReadDocument {
  path: string;
  origin: string;
  citation_numbers: number[];
  passages_read: number;
  reliable: boolean;
}

export interface ReadingClaim {
  text: string;
  evidence: Evidence;
}

/** Lectura de los documentos ya citados. Acompaña a la respuesta; no la sustituye. */
export interface AnswerReading {
  text: string;
  documents: ReadDocument[];
  claims: ReadingClaim[];
  documents_matched: number;
  documents_read: number;
  truncated: boolean;
}

export interface Answer {
  text: string;
  mode: "local";
  verified: boolean;
  citations: Evidence[];
  warning: string | null;
  used_context: boolean;
  scope: AnswerScope | null;
  clarification: Clarification | null;
  reading?: AnswerReading | null;
}

export interface ConceptSummary {
  key: string;
  display_name: string;
  value_type: string;
  occurrences: number;
}

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const desktopOnly = <T,>(operation: () => Promise<T>, preview: T): Promise<T> => inTauri ? operation() : Promise.resolve(preview);
const mutation = <T,>(operation: () => Promise<T>): Promise<T> => {
  if (!inTauri) return Promise.reject("Esta acción solo está disponible en la aplicación de escritorio.");
  return operation();
};

export const api = {
  status: () => desktopOnly(() => invoke<AppStatus>("get_status"), {
    sources: 0, documents: 0, concepts: 0, values: 0
  }),
  sources: () => desktopOnly(() => invoke<SourceSummary[]>("list_sources"), []),
  selectFolder: () => mutation(() => open({ directory: true, multiple: false, title: "Autorizar una carpeta en Omega" })),
  authorize: (path: string) => mutation(() => invoke<number>("authorize_source", { path })),
  index: (sourceId: number) => mutation(() => invoke<IndexReport>("index_source", { sourceId })),
  revoke: (sourceId: number) => mutation(() => invoke<void>("revoke_source", { sourceId })),
  concepts: (query?: string) => desktopOnly(() => invoke<ConceptSummary[]>("list_concepts", { query: query || null }), []),
  ask: (conversation: string, question: string) =>
    mutation(() => invoke<Answer>("ask_in_conversation", { conversation, question })),
  resetConversation: (conversation: string) =>
    mutation(() => invoke<void>("reset_conversation", { conversation })),
  openDocument: (path: string) => mutation(() => invoke<void>("open_document", { path }))
};

export function displayError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Ocurrió un error inesperado.";
}
