import { Fragment, FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { Answer, AnswerScope, AppStatus, ConceptSummary, IndexReport, SourceSummary, ViewName, api, displayError } from "./api";

const EMPTY_STATUS: AppStatus = {
  sources: 0,
  documents: 0,
  concepts: 0,
  values: 0
};

interface ChatItem {
  id: number;
  role: "user" | "omega";
  text: string;
  answer?: Answer;
}

export default function App() {
  const [view, setView] = useState<ViewName>("conversation");
  const [status, setStatus] = useState<AppStatus>(EMPTY_STATUS);
  const [sources, setSources] = useState<SourceSummary[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [thread, setThread] = useState(0);
  const [conversation, setConversation] = useState(() => newConversationId());
  const [sidebarOpen, setSidebarOpen] = useState(true);

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextSources] = await Promise.all([api.status(), api.sources()]);
      setStatus(nextStatus);
      setSources(nextSources);
      setError(null);
    } catch (reason) {
      setError(displayError(reason));
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);

  return (
    <div className={`app-shell ${sidebarOpen ? "" : "sidebar-hidden"}`}>
      {sidebarOpen && <Sidebar active={view} sources={sources} onNavigate={setView}
        onNewConversation={() => {
          // Borrar el contexto es una operación del motor, no de la interfaz:
          // el hilo visual y la memoria del backend se reinician juntos.
          void api.resetConversation(conversation).catch(() => undefined);
          setConversation(newConversationId());
          setThread((n) => n + 1);
          setView("conversation");
        }} />}
      <main className="workspace">
        <Topbar status={status} sidebarOpen={sidebarOpen} onToggleSidebar={() => setSidebarOpen((open) => !open)} />
        {error && <Toast message={error} onClose={() => setError(null)} />}
        {view === "conversation" && <Conversation key={thread} conversation={conversation} status={status} sources={sources} onNavigate={setView} onError={setError} />}
        {view === "sources" && <Sources sources={sources} onChanged={refresh} onError={setError} />}
        {view === "settings" && <Settings status={status} onError={setError} />}
      </main>
    </div>
  );
}

function Sidebar({ active, sources, onNavigate, onNewConversation }: { active: ViewName; sources: SourceSummary[]; onNavigate: (view: ViewName) => void; onNewConversation: () => void }) {
  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark">Ω</div>
        <span>Omega</span>
      </div>
      <div className="sidebar-actions">
        <button type="button" className="new-chat" onClick={onNewConversation}><PlusIcon /> Nueva conversación</button>
      </div>
      <nav>
        <NavButton active={active === "conversation"} icon={<ChatIcon />} label="Conversación" onClick={() => onNavigate("conversation")} />
        <NavButton active={active === "sources"} icon={<FolderIcon />} label="Fuentes" onClick={() => onNavigate("sources")} />
        <NavButton active={active === "settings"} icon={<SettingsIcon />} label="Configuración" onClick={() => onNavigate("settings")} />
      </nav>
      {sources.length > 0 && (
        <div className="sidebar-sources">
          <div className="sidebar-heading">Fuentes</div>
          {sources.map((source) => (
            <button key={source.id} type="button" className="sidebar-source" title={source.path} onClick={() => onNavigate("sources")}>
              <span className="source-chip"><FolderIcon /></span>
              <span className="sidebar-source-name">{fileName(source.path)}</span>
              <em>{source.document_count}</em>
            </button>
          ))}
        </div>
      )}
      <div className="sidebar-spacer" />
      <div className="privacy-card">
        <ShieldIcon />
        <span>Privado por diseño — recuperación local, sin salir del equipo.</span>
      </div>
    </aside>
  );
}

function NavButton({ active, icon, label, onClick }: { active: boolean; icon: ReactNode; label: string; onClick: () => void }) {
  return <button aria-label={label} className={`nav-button ${active ? "active" : ""}`} onClick={onClick}>{icon}<span>{label}</span></button>;
}

function Topbar({ status, sidebarOpen, onToggleSidebar }: { status: AppStatus; sidebarOpen: boolean; onToggleSidebar: () => void }) {
  return (
    <header className="topbar">
      <div className="topbar-left">
        <button type="button" className="icon-button" aria-pressed={sidebarOpen}
          aria-label={sidebarOpen ? "Ocultar barra lateral" : "Mostrar barra lateral"}
          title={sidebarOpen ? "Ocultar barra lateral" : "Mostrar barra lateral"}
          onClick={onToggleSidebar}><SidebarIcon /></button>
        <div className="breadcrumb"><span>Espacio local</span><i>/</i><strong>{status.documents.toLocaleString("es-MX")} documentos</strong></div>
      </div>
      <div className="engine-badge"><span className="pulse" />Recuperación con evidencia</div>
    </header>
  );
}

function Conversation({ conversation, status, sources, onNavigate, onError }: { conversation: string; status: AppStatus; sources: SourceSummary[]; onNavigate: (view: ViewName) => void; onError: (error: string) => void }) {
  const [question, setQuestion] = useState("");
  const [busy, setBusy] = useState(false);
  const [messages, setMessages] = useState<ChatItem[]>([]);
  const ready = status.documents > 0;
  const context = sources.length === 0 ? "Sin fuentes" : sources.length === 1 ? fileName(sources[0].path) : `${sources.length} fuentes`;

  async function submit(event: FormEvent) {
    event.preventDefault();
    const text = question.trim();
    if (!text || busy) return;
    const id = Date.now();
    setMessages((items) => [...items, { id, role: "user", text }]);
    setQuestion("");
    setBusy(true);
    try {
      const answer = await api.ask(conversation, text);
      setMessages((items) => [...items, { id: id + 1, role: "omega", text: answer.text, answer }]);
    } catch (reason) {
      onError(displayError(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="conversation-page">
      {messages.length === 0 ? (
        <div className="welcome">
          <div className="welcome-icon"><SearchIcon /></div>
          <h1>¿Qué quieres saber de tus documentos?</h1>
          <p>Omega responde solo lo que puede respaldar con una fuente citada.</p>
          {!ready && <button className="primary-action" onClick={() => onNavigate("sources")}><FolderIcon /> Añadir mi primera carpeta</button>}
          {ready && <PromptIdeas onSelect={setQuestion} />}
        </div>
      ) : (
        <div className="message-list">
          {messages.map((message) => <Message key={message.id} item={message} />)}
          {busy && <div className="thinking"><span /><span /><span /> Consultando el acervo local…</div>}
        </div>
      )}
      <form className="composer" onSubmit={submit}>
        <div className="composer-context">
          <span><FolderIcon /><em>{context}</em></span>
          <span><DocumentIcon /><em>{status.documents.toLocaleString("es-MX")} documentos</em></span>
        </div>
        <div className="composer-inner">
          <textarea value={question} onChange={(event) => setQuestion(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit(); } }}
            placeholder={ready ? "Pregunta algo sobre tus documentos…" : "Añade una fuente para empezar…"} disabled={!ready || busy} rows={2} />
          <div className="composer-controls">
            <button type="button" className="composer-add" aria-label="Gestionar fuentes" title="Gestionar fuentes" onClick={() => onNavigate("sources")}><PlusIcon /></button>
            <span className="engine-pill" title="Motor local activo">Local</span>
            <button type="submit" className="composer-send" disabled={!ready || !question.trim() || busy} aria-label="Enviar"><ArrowIcon /></button>
          </div>
        </div>
        <span>Omega solo afirma lo que puede respaldar con una fuente.</span>
      </form>
    </section>
  );
}

function PromptIdeas({ onSelect }: { onSelect: (value: string) => void }) {
  return <div className="prompt-grid">
    {["Encuentra un documento por identificador", "Busca documentos por estado", "Muestra la evidencia de una categoría"].map((item) => (
      <button key={item} type="button" onClick={() => onSelect(item)}>{item}</button>
    ))}
  </div>;
}

function Message({ item }: { item: ChatItem }) {
  if (item.role === "user") return <div className="user-message">{item.text}</div>;
  const answer = item.answer!;
  // Un cálculo cita su nota local y una muestra de operandos; el resto se pide
  // a propósito.
  const isCalculation = answer.citations.some((citation) => citation.match_kind === "cálculo");
  const [visibleResults, setVisibleResults] = useState(isCalculation ? 8 : 20);
  const visibleCitations = answer.citations.slice(0, visibleResults);
  return (
    <article className="omega-message">
      <div className="answer-heading"><div className="mini-mark">Ω</div><strong>Omega</strong>{answer.verified && <span className="verified"><CheckIcon /> Verificada</span>}{answer.used_context && <span className="context-badge" title="Esta respuesta continúa el resultado anterior de la conversación"><LinkIcon /> Usa el contexto anterior</span>}<em>Local</em></div>
      {/* Una aclaración se muestra sólo en su bloque: repetirla en el cuerpo
          obligaba a leer dos veces la misma pregunta. */}
      {!answer.clarification && <div className="answer-body"><MarkdownLite text={item.text} /></div>}
      {answer.scope && <ScopeChips scope={answer.scope} />}
      {answer.clarification && <div className="clarification"><strong>Necesito una aclaración</strong><p>{answer.clarification.question}</p>{answer.clarification.options.length > 0 && <ul>{answer.clarification.options.map((option) => <li key={option}>{option}</li>)}</ul>}</div>}
      {answer.warning && <div className="answer-warning">{answer.warning}</div>}
      {answer.citations.length > 0 && (
        <div className="citations"><h3>Documentos y evidencia</h3>{visibleCitations.map((source, index) => (
          <button key={source.id} onClick={() => void api.openDocument(source.path)}>
            <span>{index + 1}</span><div><strong>{fileName(source.path)}</strong><small>{source.match_kind} · {source.origin} · {source.location}{source.field ? ` · ${source.field}` : ""}</small>{source.value && <small>Valor: {source.value}{source.normalized_value ? ` · Canónico: ${source.normalized_value}` : ""}</small>}<small className="evidence-excerpt">{highlight(source.excerpt, source.matched)}</small>{!source.reliable && <small className="evidence-warning">OCR de baja confianza</small>}</div><ExternalIcon />
          </button>
        ))}{visibleResults < answer.citations.length && <button className="quiet-button" onClick={() => setVisibleResults((count) => count + 20)}>Ver más evidencia ({answer.citations.length - visibleResults})</button>}</div>
      )}
    </article>
  );
}

/** Filtros, periodo y tamaño del conjunto que produjeron la respuesta. Es el
 * mismo dato que consultó el motor, no un resumen escrito aparte. */
function ScopeChips({ scope }: { scope: AnswerScope }) {
  const chips: string[] = [];
  if (scope.inherited) chips.push("Resultado anterior");
  if (scope.origin) chips.push(`Carpeta: ${scope.origin}`);
  scope.filters.forEach((filter) => chips.push(`${filter.concept}: ${filter.equals}`));
  if (scope.date) chips.push(`${scope.date.concept}: ${scope.date.from} → ${scope.date.to}`);
  if (scope.concept) chips.push(`Campo: ${scope.concept}`);
  if (scope.group_by) chips.push(`Agrupado por: ${scope.group_by}`);
  if (scope.document_count !== null) chips.push(`${scope.document_count.toLocaleString("es-MX")} documentos`);
  if (scope.value_count !== null) chips.push(`${scope.value_count.toLocaleString("es-MX")} valores`);
  if (chips.length === 0) return null;
  return <div className="scope-chips">{chips.map((chip) => <span key={chip}>{chip}</span>)}</div>;
}

/// Renderizador mínimo para el texto que redacta `answer.rs`: reconoce
/// negritas, listas (con viñeta y numeradas) y tablas simples separadas por
/// líneas en blanco. No es un parser de Markdown general — sólo cubre lo que
/// el backend puede llegar a producir — así el proyecto no suma una
/// dependencia externa sólo para mostrar listas y tablas cortas.
function MarkdownLite({ text }: { text: string }) {
  const blocks = text.split(/\n{2,}/).map((block) => block.trim()).filter(Boolean);
  return <>{blocks.map((block, index) => <MarkdownBlock key={index} block={block} lead={index === 0} />)}</>;
}

function MarkdownBlock({ block, lead }: { block: string; lead: boolean }) {
  const lines = block.split("\n").map((line) => line.trim()).filter(Boolean);
  if (lines.length > 1 && lines.every((line) => line.startsWith("|"))) return <MarkdownTable lines={lines} />;
  if (lines.length > 0 && lines.every((line) => /^-\s/.test(line))) {
    return <ul className="answer-list">{lines.map((line, i) => <li key={i}>{inlineMarkdown(line.replace(/^-\s+/, ""))}</li>)}</ul>;
  }
  if (lines.length > 0 && lines.every((line) => /^\d+\.\s/.test(line))) {
    return <ol className="answer-list">{lines.map((line, i) => <li key={i}>{inlineMarkdown(line.replace(/^\d+\.\s+/, ""))}</li>)}</ol>;
  }
  const joined = lines.join(" ");
  if (/^\+\s?\d/.test(joined)) return <p className="answer-note">{inlineMarkdown(joined)}</p>;
  return <p className={lead ? "answer-lead" : undefined}>{inlineMarkdown(joined)}</p>;
}

function MarkdownTable({ lines }: { lines: string[] }) {
  const rows = lines.map((line) => line.replace(/^\|/, "").replace(/\|$/, "").split("|").map((cell) => cell.trim()));
  const [header, divider, ...rest] = rows;
  const body = divider?.every((cell) => /^-+$/.test(cell)) ? rest : rows.slice(1);
  return (
    <div className="answer-table-wrap">
      <table className="answer-table">
        <thead><tr>{header.map((cell, i) => <th key={i}>{inlineMarkdown(cell)}</th>)}</tr></thead>
        <tbody>{body.map((row, i) => <tr key={i}>{row.map((cell, j) => <td key={j}>{inlineMarkdown(cell)}</td>)}</tr>)}</tbody>
      </table>
    </div>
  );
}

function inlineMarkdown(text: string): ReactNode {
  return text.split(/(\*\*[^*]+\*\*)/g).filter(Boolean).map((part, i) =>
    part.startsWith("**") && part.endsWith("**")
      ? <strong key={i}>{part.slice(2, -2)}</strong>
      : <Fragment key={i}>{part}</Fragment>
  );
}

function Sources({ sources, onChanged, onError }: { sources: SourceSummary[]; onChanged: () => Promise<void>; onError: (error: string) => void }) {
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState<number | "new" | null>(null);
  const [report, setReport] = useState<IndexReport | null>(null);

  async function add(event: FormEvent) {
    event.preventDefault();
    if (!path.trim()) return;
    setBusy("new");
    try {
      const id = await api.authorize(path.trim());
      const next = await api.index(id);
      setReport(next);
      setPath("");
      await onChanged();
    } catch (reason) { onError(displayError(reason)); }
    finally { setBusy(null); }
  }

  async function chooseFolder() {
    try {
      const selected = await api.selectFolder();
      if (typeof selected === "string") setPath(selected);
    } catch (reason) {
      onError(displayError(reason));
    }
  }

  async function reindex(id: number) {
    setBusy(id);
    try { setReport(await api.index(id)); await onChanged(); }
    catch (reason) { onError(displayError(reason)); }
    finally { setBusy(null); }
  }

  async function revoke(source: SourceSummary) {
    if (!window.confirm(`¿Revocar ${source.path}? El índice derivado se eliminará de inmediato.`)) return;
    setBusy(source.id);
    try { await api.revoke(source.id); setReport(null); await onChanged(); }
    catch (reason) { onError(displayError(reason)); }
    finally { setBusy(null); }
  }

  return (
    <section className="page content-page">
      <PageTitle eyebrow="Acervo autorizado" title="Fuentes documentales" description="Omega lee estas carpetas sin modificar sus archivos. Revocar una fuente elimina inmediatamente todo dato derivado." />
      <form className="source-form" onSubmit={add}>
        <FolderIcon /><input readOnly value={path} onClick={() => void chooseFolder()} placeholder="Selecciona una carpeta local…" />
        <button type="button" className="picker-button" disabled={busy !== null} onClick={() => void chooseFolder()}>Elegir carpeta</button>
        <button disabled={!path.trim() || busy !== null}>{busy === "new" ? "Indexando…" : "Autorizar e indexar"}</button>
      </form>
      {report && <><div className="report-strip"><CheckIcon /><strong>{report.indexed} documentos indexados</strong>{report.modified > 0 && <span>{report.modified} modificados</span>}<span>{report.values.toLocaleString("es-MX")} valores con evidencia</span><span>{report.elapsed_ms} ms</span>{report.ocr_pending > 0 && <span>{report.ocr_pending} pendientes de OCR</span>}</div>{report.warnings.map((warning) => <div key={warning} className="answer-warning">{warning}</div>)}</>}
      {sources.length === 0 && <EmptyCard icon={<FolderIcon />} title="Aún no hay fuentes" text="Autoriza una carpeta local de documentos para comenzar." />}
      {sources.length > 0 && <div className="section-label">Autorizadas</div>}
      {sources.length > 0 && <div className="source-list">
        {sources.map((source) => (
          <article key={source.id} className="source-card">
            <div className="source-icon"><FolderIcon /></div>
            <div className="source-copy"><strong>{fileName(source.path)}</strong><span>{source.path}</span><small>{source.document_count} documentos · {source.indexed_at ? `Indexada ${formatDate(source.indexed_at)}` : "Pendiente"}</small></div>
            <button className="quiet-button" disabled={busy !== null} onClick={() => void reindex(source.id)}>{busy === source.id ? "Trabajando…" : "Reindexar"}</button>
            <button className="danger-button" disabled={busy !== null} onClick={() => void revoke(source)}>Revocar</button>
          </article>
        ))}
      </div>}
    </section>
  );
}

function Settings({ status, onError }: { status: AppStatus; onError: (error: string) => void }) {
  const [concepts, setConcepts] = useState<ConceptSummary[]>([]);
  const [showConcepts, setShowConcepts] = useState(false);

  async function loadConcepts() {
    setShowConcepts((current) => !current);
    if (concepts.length === 0) {
      try { setConcepts(await api.concepts()); } catch (reason) { onError(displayError(reason)); }
    }
  }

  return (
    <section className="page content-page settings-page">
      <PageTitle eyebrow="Control y privacidad" title="Configuración" description="La búsqueda, los cálculos y las respuestas verificadas se realizan únicamente en este equipo." />
      <div className="settings-grid">
        <section className="setting-card featured">
          <div className="setting-title"><div className="setting-icon"><ShieldIcon /></div><div><h2>Motor local</h2><p>Omega analiza únicamente las carpetas que autorizas y responde con evidencia citable.</p></div></div>
          <div className="privacy-note"><ShieldIcon /><p>No usa modelos remotos, API, claves ni conexión de red. Tus archivos y preguntas permanecen en este equipo.</p></div>
        </section>
        <section className="setting-card">
          <div className="setting-title"><div className="setting-icon muted"><DatabaseIcon /></div><div><h2>Catálogo descubierto</h2><p>{status.concepts.toLocaleString("es-MX")} conceptos · {status.values.toLocaleString("es-MX")} valores clasificados</p></div><button className="quiet-button" onClick={() => void loadConcepts()}>{showConcepts ? "Ocultar" : "Ver conceptos"}</button></div>
          {showConcepts && <div className="concept-cloud">{concepts.slice(0, 36).map((concept) => <span key={concept.key}>{concept.display_name}<em>{concept.occurrences}</em></span>)}</div>}
        </section>
      </div>
    </section>
  );
}

function PageTitle({ eyebrow, title, description }: { eyebrow: string; title: string; description: string }) {
  return <header className="page-title"><span>{eyebrow}</span><h1>{title}</h1><p>{description}</p></header>;
}

function EmptyCard({ icon, title, text }: { icon: ReactNode; title: string; text: string }) {
  return <div className="empty-card"><div>{icon}</div><strong>{title}</strong><span>{text}</span></div>;
}

function Toast({ message, onClose }: { message: string; onClose: () => void }) {
  return <div className="toast"><span>!</span><p>{message}</p><button onClick={onClose}>×</button></div>;
}

function newConversationId() {
  return `conv-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}
function fileName(path: string) { return path.split(/[\\/]/).filter(Boolean).pop() ?? path; }
function highlight(text: string, matched: string | null) {
  if (!matched?.trim()) return text;
  const index = text.toLocaleLowerCase("es").indexOf(matched.toLocaleLowerCase("es"));
  if (index < 0) return text;
  return <>{text.slice(0, index)}<mark>{text.slice(index, index + matched.length)}</mark>{text.slice(index + matched.length)}</>;
}
function formatDate(value: string) { const date = new Date(`${value.replace(" ", "T")}Z`); return Number.isNaN(date.valueOf()) ? value : date.toLocaleString("es-MX", { dateStyle: "medium", timeStyle: "short" }); }

const icon = (children: ReactNode) => <svg viewBox="0 0 24 24" aria-hidden="true">{children}</svg>;
function ChatIcon() { return icon(<><path d="M5 5.5h14v10H9l-4 3v-13Z" /><path d="M8 9h8M8 12h5" /></>); }
function FolderIcon() { return icon(<path d="M3.5 6.5h6l2 2h9v9h-17v-11Z" />); }
function SettingsIcon() { return icon(<><circle cx="12" cy="12" r="3" /><path d="M12 3.5v2M12 18.5v2M20.5 12h-2M5.5 12h-2M18 6l-1.5 1.5M7.5 16.5 6 18M18 18l-1.5-1.5M7.5 7.5 6 6" /></>); }
function ShieldIcon() { return icon(<><path d="M12 3 5 6v5c0 4.5 2.5 7.5 7 10 4.5-2.5 7-5.5 7-10V6l-7-3Z" /><path d="m9 12 2 2 4-4" /></>); }
function PlusIcon() { return icon(<path d="M12 5v14M5 12h14" />); }
function SearchIcon() { return icon(<><circle cx="11" cy="11" r="6.5" /><path d="m20 20-3.6-3.6" /><path d="M8.5 11h5M11 8.5v5" /></>); }
function SidebarIcon() { return icon(<><rect x="3" y="5" width="18" height="14" rx="2.5" /><path d="M9.5 5v14" /></>); }
function DocumentIcon() { return icon(<><path d="M14 3.5H7.5A1.5 1.5 0 0 0 6 5v14a1.5 1.5 0 0 0 1.5 1.5h9A1.5 1.5 0 0 0 18 19V7.5L14 3.5Z" /><path d="M13.5 3.5V8H18" /></>); }
function ArrowIcon() { return icon(<><path d="M5 12h14M14 7l5 5-5 5" /></>); }
function CheckIcon() { return icon(<path d="m5 12 4 4L19 6" />); }
function ExternalIcon() { return icon(<><path d="M13 5h6v6M19 5l-8 8" /><path d="M17 13v5H6V7h5" /></>); }
function LinkIcon() { return icon(<><path d="M10 13.5a3.5 3.5 0 0 0 5 0l2.5-2.5a3.5 3.5 0 0 0-5-5L11 7.5" /><path d="M14 10.5a3.5 3.5 0 0 0-5 0L6.5 13a3.5 3.5 0 0 0 5 5l1.5-1.5" /></>); }
function DatabaseIcon() { return icon(<><ellipse cx="12" cy="5.5" rx="7" ry="3" /><path d="M5 5.5v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6M5 11.5v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6" /></>); }
