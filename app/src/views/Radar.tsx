import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Console } from "../App";
import { ago } from "../api";
import { Empty, Panel, Pip, Rich } from "../ui";

type Competitor = {
  id: number;
  slug: string;
  name: string;
  url: string;
  last_checked: string | null;
  last_error: string | null;
};

type Event = { id: number; slug: string; competitor: string; url: string; summary: string; detail: string; at: string };

/** Radar: páginas de la competencia que Atalaya lee cada día para avisar cuando cambian. */
export function Radar({ contexts }: Console) {
  const [competitors, setCompetitors] = useState<Competitor[]>([]);
  const [events, setEvents] = useState<Event[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState<number | null>(null);
  const [slug, setSlug] = useState("");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");

  const names = Object.fromEntries(contexts.map((c) => [c.project.slug, c.project.name]));

  const load = useCallback(async () => {
    try {
      const data = await invoke<{ competitors: Competitor[]; events: Event[] }>("radar");
      setCompetitors(data.competitors);
      setEvents(data.events);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  async function act(task: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await task();
      await load();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function add(event: React.FormEvent) {
    event.preventDefault();
    const target = url.trim().startsWith("http") ? url.trim() : `https://${url.trim()}`;
    if (await act(() => invoke("radar_add", { slug: slug || contexts[0]?.project.slug, name: name.trim(), url: target }))) {
      setName("");
      setUrl("");
    }
  }

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Radar</h1>
          <p>
            Páginas de tu competencia, por ejemplo sus precios. Atalaya las lee una vez al día y, cuando cambian, te resume
            qué cambió.
          </p>
        </div>
        <button className="btn" disabled={busy || !competitors.length} onClick={() => act(() => invoke("radar_check"))}>
          {busy ? "Revisando…" : "Revisar ahora"}
        </button>
      </header>

      {error && <div className="banner">{error}</div>}

      <div className="with-aside">
        <Panel title="Cambios detectados" aside={`${events.length} registrados`} flush>
          {events.length ? (
            <ul className="radar-events">
              {events.map((e) => (
                <li key={e.id}>
                  <div className="alert-top">
                    <span className="sev almenara">{e.competitor}</span>
                    <span className="polvo">· {names[e.slug] ?? e.slug}</span>
                    <span className="stamp">{ago(e.at)}</span>
                  </div>
                  <div className="radar-summary">
                    <Rich text={e.summary} />
                  </div>
                  <div className="card-actions left">
                    <button className="link" onClick={() => setOpen(open === e.id ? null : e.id)}>
                      {open === e.id ? "Ocultar líneas" : "Ver qué líneas cambiaron"}
                    </button>
                    <button className="link" onClick={() => openUrl(e.url)}>
                      Abrir la página ↗
                    </button>
                  </div>
                  {open === e.id && <pre className="doc diff">{e.detail}</pre>}
                </li>
              ))}
            </ul>
          ) : (
            <div className="panel-body">
              <Empty>
                {competitors.length
                  ? "Sin cambios todavía. La primera lectura de cada página queda como referencia."
                  : "Agrega una página a la derecha para empezar a vigilarla."}
              </Empty>
            </div>
          )}
        </Panel>

        <Panel title="Páginas vigiladas" aside={String(competitors.length)}>
          {competitors.length > 0 && (
            <ul className="rules">
              {competitors.map((c) => (
                <li key={c.id}>
                  <div className="competitor">
                    <span className="cell-name">
                      <Pip status={c.last_error ? "falla" : c.last_checked ? "ok" : undefined} />
                      {c.name}
                    </span>
                    <button className="icon-btn" title="Dejar de vigilar" disabled={busy} onClick={() => act(() => invoke("radar_remove", { id: c.id }))}>
                      ×
                    </button>
                  </div>
                  <div className="readout">{c.url}</div>
                  <div className={c.last_error ? "stamp brasa" : "stamp"}>
                    {names[c.slug] ?? c.slug} · {c.last_error ?? (c.last_checked ? `leída ${ago(c.last_checked)}` : "sin leer")}
                  </div>
                </li>
              ))}
            </ul>
          )}
          <form className="form compact" onSubmit={add}>
            <label>
              Proyecto
              <select value={slug || contexts[0]?.project.slug} onChange={(e) => setSlug(e.target.value)}>
                {contexts.map((c) => (
                  <option key={c.project.slug} value={c.project.slug}>
                    {c.project.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              Competidor
              <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Nombre" />
            </label>
            <label>
              Página a vigilar
              <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://competidor.com/precios" spellCheck={false} />
            </label>
            <button className="btn primary" disabled={busy || !name.trim() || !url.trim() || !contexts.length}>
              Vigilar
            </button>
          </form>
          <p className="footnote">Solo lee páginas públicas. Las que se arman con JavaScript pueden no traer texto suficiente.</p>
        </Panel>
      </div>
    </div>
  );
}
