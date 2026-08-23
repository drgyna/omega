import { FormEvent, ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { Answer, AppStatus, ConceptSummary, IndexReport, SourceSummary, ViewName, api, displayError } from "./api";

const EMPTY_STATUS: AppStatus = {
  sources: 0,
  documents: 0,
  concepts: 0,
  values: 0,
  ai_enabled: false,
  api_key_stored: false
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
    <div className="app-shell">
      <Sidebar active={view} status={status} onNavigate={setView} />
      <main className="workspace">
        <Topbar status={status} />
        {error && <Toast message={error} onClose={() => setError(null)} />}
        {view === "conversation" && <Conversation status={status} onNavigate={setView} onError={setError} />}
        {view === "sources" && <Sources sources={sources} onChanged={refresh} onError={setError} />}
        {view === "settings" && <Settings status={status} onChanged={refresh} onError={setError} />}
      </main>
    </div>
  );
}

function Sidebar({ active, status, onNavigate }: { active: ViewName; status: AppStatus; onNavigate: (view: ViewName) => void }) {
  return (
    <aside className="sidebar">
      <div className="brand">
        <div className="brand-mark">Ω</div>
        <div><strong>OMEGA</strong><span>Inteligencia documental</span></div>
      </div>
      <nav>
        <NavButton active={active === "conversation"} icon={<ChatIcon />} label="Conversación" onClick={() => onNavigate("conversation")} />
        <NavButton active={active === "sources"} icon={<FolderIcon />} label="Fuentes" count={status.sources} onClick={() => onNavigate("sources")} />
        <NavButton active={active === "settings"} icon={<SettingsIcon />} label="Configuración" onClick={() => onNavigate("settings")} />
      </nav>
      <div className="privacy-card">
        <ShieldIcon />
        <div><strong>Privado por diseño</strong><span>Recuperación local con evidencia</span></div>
      </div>
      <div className="sidebar-foot"><span className="status-dot" /> Motor de recuperación disponible</div>
    </aside>
  );
}

function NavButton({ active, icon, label, count, onClick }: { active: boolean; icon: ReactNode; label: string; count?: number; onClick: () => void }) {
  return <button aria-label={label} className={`nav-button ${active ? "active" : ""}`} onClick={onClick}>{icon}<span>{label}</span>{count !== undefined && <em>{count}</em>}</button>;
}

function Topbar({ status }: { status: AppStatus }) {
  return (
    <header className="topbar">
      <div className="breadcrumb"><span>Espacio local</span><i>/</i><strong>{status.documents.toLocaleString("es-MX")} documentos</strong></div>
      <div className="engine-badge"><span className="pulse" />Recuperación con evidencia</div>
    </header>
  );
}

function Conversation({ status, onNavigate, onError }: { status: AppStatus; onNavigate: (view: ViewName) => void; onError: (error: string) => void }) {
  const [question, setQuestion] = useState("");
  const [busy, setBusy] = useState(false);
  const [messages, setMessages] = useState<ChatItem[]>([]);
  const ready = status.documents > 0;

  async function submit(event: FormEvent) {
    event.preventDefault();
    const text = question.trim();
    if (!text || busy) return;
    const id = Date.now();
    setMessages((items) => [...items, { id, role: "user", text }]);
    setQuestion("");
    setBusy(true);
    try {
      const answer = await api.ask(text);
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
          <div className="eyebrow"><span /> Pregunta con evidencia</div>
          <h1>Tu negocio, <em>en una conversación.</em></h1>
          <p>Omega busca, calcula y responde usando únicamente tus documentos autorizados. Cada dato importante conserva su fuente.</p>
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
        <div className="composer-inner">
          <textarea value={question} onChange={(event) => setQuestion(event.target.value)}
            onKeyDown={(event) => { if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); event.currentTarget.form?.requestSubmit(); } }}
            placeholder={ready ? "Pregunta algo sobre tus documentos…" : "Añade una fuente para empezar…"} disabled={!ready || busy} rows={2} />
          <button type="submit" disabled={!ready || !question.trim() || busy} aria-label="Enviar"><ArrowIcon /></button>
        </div>
        <span>Omega solo afirma lo que puede respaldar con una fuente.</span>
      </form>
    </section>
  );
}

function PromptIdeas({ onSelect }: { onSelect: (value: string) => void }) {
  return <div className="prompt-grid">
    {["Encuentra un documento por identificador", "Busca documentos por estado", "Muestra la evidencia de una categoría"].map((item) => (
      <button key={item} onClick={() => onSelect(item)}><SparkIcon /><span>{item}</span><ArrowIcon /></button>
    ))}
  </div>;
}

function Message({ item }: { item: ChatItem }) {
  if (item.role === "user") return <div className="user-message">{item.text}</div>;
  const answer = item.answer!;
  const [visibleResults, setVisibleResults] = useState(20);
  const visibleCitations = answer.citations.slice(0, visibleResults);
  return (
    <article className="omega-message">
      <div className="answer-heading"><div className="mini-mark">Ω</div><strong>Omega</strong><span className="verified"><CheckIcon /> Verificada</span><em>{answer.mode === "ai" ? "IA" : "Local"}</em></div>
      <p>{item.text}</p>
      {answer.warning && <div className="answer-warning">{answer.warning}</div>}
      {answer.citations.length > 0 && (
        <div className="citations"><h3>Documentos y evidencia</h3>{visibleCitations.map((source, index) => (
          <button key={source.id} onClick={() => void api.openDocument(source.path)}>
            <span>{index + 1}</span><div><strong>{fileName(source.path)}</strong><small>{source.match_kind} · {source.origin} · {source.location}{source.field ? ` · ${source.field}` : ""}</small>{source.value && <small>Valor: {source.value}{source.normalized_value ? ` · Canónico: ${source.normalized_value}` : ""}</small>}<small className="evidence-excerpt">{highlight(source.excerpt, source.matched)}</small>{!source.reliable && <small className="evidence-warning">OCR de baja confianza</small>}</div><ExternalIcon />
          </button>
        ))}{visibleResults < answer.citations.length && <button className="quiet-button" onClick={() => setVisibleResults((count) => count + 20)}>Ver más resultados</button>}</div>
      )}
    </article>
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
      <div className="source-list">
        {sources.length === 0 && <EmptyCard icon={<FolderIcon />} title="Aún no hay fuentes" text="Autoriza una carpeta local de documentos para comenzar." />}
        {sources.map((source) => (
          <article key={source.id} className="source-card">
            <div className="source-icon"><FolderIcon /></div>
            <div className="source-copy"><strong>{fileName(source.path)}</strong><span>{source.path}</span><small>{source.document_count} documentos · {source.indexed_at ? `Indexada ${formatDate(source.indexed_at)}` : "Pendiente"}</small></div>
            <button className="quiet-button" disabled={busy !== null} onClick={() => void reindex(source.id)}>{busy === source.id ? "Trabajando…" : "Reindexar"}</button>
            <button className="danger-button" disabled={busy !== null} onClick={() => void revoke(source)}>Revocar</button>
          </article>
        ))}
      </div>
    </section>
  );
}

function Settings({ status, onChanged, onError }: { status: AppStatus; onChanged: () => Promise<void>; onError: (error: string) => void }) {
  const [consent, setConsent] = useState(false);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [concepts, setConcepts] = useState<ConceptSummary[]>([]);
  const [showConcepts, setShowConcepts] = useState(false);

  async function toggleAi(enabled: boolean) {
    if (enabled && !consent) { onError("Marca primero el consentimiento explícito."); return; }
    setBusy(true);
    try { await api.configureAi(enabled, enabled ? consent : false); await onChanged(); }
    catch (reason) { onError(displayError(reason)); }
    finally { setBusy(false); }
  }

  async function saveKey(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    try { await api.storeApiKey(key); setKey(""); await onChanged(); }
    catch (reason) { onError(displayError(reason)); }
    finally { setBusy(false); }
  }

  async function loadConcepts() {
    setShowConcepts((current) => !current);
    if (concepts.length === 0) {
      try { setConcepts(await api.concepts()); } catch (reason) { onError(displayError(reason)); }
    }
  }

  return (
    <section className="page content-page settings-page">
      <PageTitle eyebrow="Control y privacidad" title="Configuración" description="La búsqueda y los cálculos siempre funcionan localmente. La IA es opcional y permanece apagada hasta que tú decidas activarla." />
      <div className="settings-grid">
        <section className="setting-card featured">
          <div className="setting-title"><div className="setting-icon"><SparkIcon /></div><div><h2>Comprensión con IA</h2><p>Usa GPT-5.6 para interpretar lenguaje natural y elegir herramientas locales.</p></div><Toggle checked={status.ai_enabled} disabled={busy} onChange={toggleAi} /></div>
          <div className="privacy-note"><ShieldIcon /><p>Los archivos completos nunca se envían. Solo viajan la pregunta y los fragmentos mínimos devueltos por las herramientas que el modelo solicite.</p></div>
          {!status.ai_enabled && <label className="consent"><input type="checkbox" checked={consent} onChange={(event) => setConsent(event.target.checked)} /><span>Entiendo que, al activar esta función, mi pregunta y evidencia mínima se enviarán a OpenAI.</span></label>}
          <form className="key-form" onSubmit={saveKey}><input aria-label="Clave de API de OpenAI" type="password" value={key} onChange={(event) => setKey(event.target.value)} placeholder={status.api_key_stored ? "Clave guardada de forma segura" : "sk-…"} /><button disabled={!key || busy}>Guardar en Keychain</button>{status.api_key_stored && <button type="button" className="text-button" onClick={() => void api.clearApiKey().then(onChanged).catch((reason) => onError(displayError(reason)))}>Eliminar</button>}</form>
        </section>
        <section className="setting-card">
          <div className="setting-title"><div className="setting-icon muted"><DatabaseIcon /></div><div><h2>Catálogo descubierto</h2><p>{status.concepts.toLocaleString("es-MX")} conceptos · {status.values.toLocaleString("es-MX")} valores clasificados</p></div><button className="quiet-button" onClick={() => void loadConcepts()}>{showConcepts ? "Ocultar" : "Ver conceptos"}</button></div>
          {showConcepts && <div className="concept-cloud">{concepts.slice(0, 36).map((concept) => <span key={concept.key}>{concept.display_name}<em>{concept.occurrences}</em></span>)}</div>}
        </section>
      </div>
    </section>
  );
}

function Toggle({ checked, disabled, onChange }: { checked: boolean; disabled: boolean; onChange: (checked: boolean) => void }) {
  return <button aria-label="Activar IA" aria-pressed={checked} disabled={disabled} className={`toggle ${checked ? "on" : ""}`} onClick={() => onChange(!checked)}><span /></button>;
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
function SparkIcon() { return icon(<><path d="m12 3 1.5 5.5L19 10l-5.5 1.5L12 17l-1.5-5.5L5 10l5.5-1.5L12 3Z" /><path d="m18.5 16 .7 2.3 2.3.7-2.3.7-.7 2.3-.7-2.3-2.3-.7 2.3-.7.7-2.3Z" /></>); }
function ArrowIcon() { return icon(<><path d="M5 12h14M14 7l5 5-5 5" /></>); }
function CheckIcon() { return icon(<path d="m5 12 4 4L19 6" />); }
function ExternalIcon() { return icon(<><path d="M13 5h6v6M19 5l-8 8" /><path d="M17 13v5H6V7h5" /></>); }
function DatabaseIcon() { return icon(<><ellipse cx="12" cy="5.5" rx="7" ry="3" /><path d="M5 5.5v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6M5 11.5v6c0 1.7 3.1 3 7 3s7-1.3 7-3v-6" /></>); }
