import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Console } from "../App";
import { type Improvement, type ImprovementState, ago, api } from "../api";
import { Empty, Pip, Rich, Tag } from "../ui";

/** El flujo: se propone → se acepta y el agente la trabaja → queda en revisión → hecha. */
const COLUMNS: { title: string; note: string; tone: "aviso" | "ok" | undefined; states: ImprovementState[] }[] = [
  { title: "Propuestas", note: "Por decidir", tone: "aviso", states: ["propuesta"] },
  { title: "En curso", note: "Aceptadas o en trabajo", tone: "aviso", states: ["en_curso", "aceptada"] },
  { title: "En revisión", note: "Esperan tu visto bueno", tone: "ok", states: ["en_revision"] },
  { title: "Hechas", note: "Terminadas", tone: undefined, states: ["hecha"] },
];

const SOURCE: Record<string, string> = { app: "manual", terminal: "terminal", mcp: "agente" };

/** La pregunta con la que el agente terminó sin hacer cambios, si la dejó. */
function questionOf(item: Improvement): string | null {
  if (item.status !== "aceptada" || !item.result) return null;
  const lines = item.result.split("\n").map((l) => l.trim());
  const marked = lines.find((l) => /^[*_]*pregunta:/i.test(l));
  if (marked) return marked.replace(/^[*_]*pregunta:[*_]*\s*/i, "");
  const last = lines.filter(Boolean).pop() ?? "";
  return last.endsWith("?") ? last : null;
}

type WorkEvent = { id: number; kind: "estado" | "paso" | "nota" | "fin" | "error"; text: string };

export function Mejoras({ contexts, improvements, refresh, openProject, answerInAgent }: Console) {
  const [adding, setAdding] = useState(false);
  const [showDiscarded, setShowDiscarded] = useState(false);
  const [slug, setSlug] = useState("");
  const [title, setTitle] = useState("");
  const [detail, setDetail] = useState("");
  const [error, setError] = useState<string | null>(null);
  // Última acción del agente por mejora, y cuáles esperan turno.
  const [activity, setActivity] = useState<Record<number, string>>({});
  const [queued, setQueued] = useState<number[]>([]);
  const [confirming, setConfirming] = useState<number | null>(null);
  const [merging, setMerging] = useState<number | null>(null);
  const [expanded, setExpanded] = useState<number | null>(null);
  // A qué mejora le está respondiendo el usuario, y qué lleva escrito.
  const [replying, setReplying] = useState<number | null>(null);
  const [reply, setReply] = useState("");

  const names = Object.fromEntries(contexts.map((c) => [c.project.slug, c.project.name]));
  const canWork = Object.fromEntries(contexts.map((c) => [c.project.slug, Boolean(c.project.local_path)]));
  const discarded = improvements.filter((i) => i.status === "descartada");

  useEffect(() => {
    api
      .queuedImprovements()
      .then(setQueued)
      .catch(() => {});
    const stop = listen<WorkEvent>("work", ({ payload }) => {
      if (payload.kind === "estado" || payload.kind === "paso") {
        setActivity((all) => ({ ...all, [payload.id]: payload.text.split("\n")[0] }));
      }
      if (payload.kind === "error") setError(`Mejora #${payload.id}: ${payload.text}`);
      // Cada cambio de etapa mueve la tarjeta: se vuelve a leer el tablero.
      if (payload.kind !== "paso" && payload.kind !== "nota") {
        api
          .queuedImprovements()
          .then(setQueued)
          .catch(() => {});
        refresh();
      }
    });
    return () => {
      stop.then((off) => off());
    };
  }, [refresh]);

  const act = async (task: () => Promise<unknown>) => {
    try {
      await task();
      setError(null);
      await refresh();
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    }
  };

  /** Deja la mejora aceptada y la pone en la cola del agente. */
  const work = (item: Improvement) =>
    act(async () => {
      if (item.status !== "aceptada") await api.setImprovementStatus(item.id, "aceptada");
      if (canWork[item.slug]) {
        await api.startImprovement(item.id);
        setQueued(await api.queuedImprovements());
      }
    });

  /** Le responde al agente y la mejora vuelve a la cola con esa respuesta. */
  async function answer(item: Improvement) {
    const done = await act(async () => {
      await api.answerImprovement(item.id, reply);
      if (canWork[item.slug]) {
        await api.startImprovement(item.id);
        setQueued(await api.queuedImprovements());
      }
    });
    if (done) {
      setReply("");
      setReplying(null);
    }
  }

  const replyButton = (item: Improvement, label: string) =>
    replying !== item.id && (
      <button
        key="reply"
        className="btn"
        onClick={() => {
          setReplying(item.id);
          setReply("");
        }}
      >
        {label}
      </button>
    );

  async function merge(item: Improvement) {
    setConfirming(null);
    setMerging(item.id);
    await act(() => api.mergeImprovement(item.id));
    setMerging(null);
  }

  async function add(event: React.FormEvent) {
    event.preventDefault();
    if (await act(() => api.addImprovement(slug || contexts[0]?.project.slug, title, detail))) {
      setTitle("");
      setDetail("");
      setAdding(false);
    }
  }

  const move = (item: Improvement, label: string, status: ImprovementState) => (
    <button key={status} className="btn" onClick={() => act(() => api.setImprovementStatus(item.id, status))}>
      {label}
    </button>
  );

  function actions(item: Improvement) {
    switch (item.status) {
      case "propuesta":
        return [
          <button key="go" className="btn primary" onClick={() => work(item)}>
            Aceptar
          </button>,
          move(item, "Descartar", "descartada"),
        ];
      case "aceptada":
        if (queued.includes(item.id)) return [];
        return [
          item.result && !questionOf(item) && canWork[item.slug] && replyButton(item, "Dar indicaciones"),
          item.result && (
            <button key="agent" className="btn" onClick={() => answerInAgent(item.id)} title="Claude Code la retoma en el chat">
              Responder en el Agente
            </button>
          ),
          canWork[item.slug] && (
            <button key="go" className="btn primary" onClick={() => work(item)}>
              {item.result ? "Reintentar" : "Trabajar"}
            </button>
          ),
          move(item, "Hecha a mano", "hecha"),
          move(item, "Volver", "propuesta"),
        ];
      case "en_curso":
        return [];
      case "en_revision":
        return [
          item.pr_url && (
            <button key="pr" className="btn" onClick={() => openUrl(item.pr_url!)}>
              Ver PR ↗
            </button>
          ),
          item.pr_url &&
            (confirming === item.id ? (
              <button key="merge" className="btn primary" onClick={() => merge(item)}>
                Confirmar: sale a producción
              </button>
            ) : (
              <button key="merge" className="btn primary" disabled={merging === item.id} onClick={() => setConfirming(item.id)}>
                {merging === item.id ? "Fusionando…" : "Fusionar"}
              </button>
            )),
          // Sin PR (un cambio hecho desde el chat, por ejemplo) no hay nada que fusionar: se da por hecha a mano.
          !item.pr_url && move(item, "Marcar hecha", "hecha"),
          canWork[item.slug] && replyButton(item, "Pedir cambios"),
          <button key="redo" className="btn" onClick={() => work(item)}>
            Rehacer
          </button>,
          move(item, "Descartar", "descartada"),
        ];
      case "hecha":
        return [move(item, "Reabrir", "aceptada")];
      default:
        return [move(item, "Recuperar", "propuesta")];
    }
  }

  const card = (item: Improvement) => {
    const running = item.status === "en_curso";
    const waiting = item.status === "aceptada" && queued.includes(item.id);
    const open = expanded === item.id;
    const question = questionOf(item);
    // Si el agente preguntó algo, el cuadro para responderle aparece solo.
    const answering = canWork[item.slug] && !waiting && (replying === item.id || (question !== null && replying === null));
    return (
      <article className={running ? "card live" : "card"} key={item.id}>
        <div className="card-top">
          <button className="link plain project-chip" onClick={() => openProject(item.slug)}>
            {names[item.slug] ?? item.slug}
          </button>
          <span className="stamp">
            #{item.id} · {SOURCE[item.source] ?? item.source}
          </span>
        </div>
        <h3>{item.title}</h3>
        {item.detail && <p className={open ? "" : "clamp"}>{item.detail}</p>}

        {running && (
          <div className="card-activity">
            ▸ {activity[item.id] ?? item.result?.split("\n")[0] ?? "El agente está trabajando…"}
          </div>
        )}
        {waiting && <div className="card-activity idle">▸ En cola, espera su turno</div>}
        {item.status === "aceptada" && !waiting && !canWork[item.slug] && (
          <div className="card-activity idle">Sin carpeta local: el agente no puede trabajarla aquí.</div>
        )}

        {item.result && !running && (
          <div className="card-result">
            <div className="card-result-head">
              <Tag tone={item.status === "aceptada" ? "almenara" : "liquen"}>
                {question ? "Te pregunta" : item.status === "aceptada" ? "Último intento" : "Informe del agente"}
              </Tag>
              {item.branch && item.status !== "aceptada" && <span className="stamp">{item.branch}</span>}
            </div>
            {question && <p className="card-question">{question}</p>}
            <p className={open ? "" : "clamp"}>
              <Rich text={item.result} />
            </p>
          </div>
        )}
        {open && item.feedback && (
          <div className="card-result">
            <div className="card-result-head">
              <Tag>Lo que le has respondido</Tag>
            </div>
            <p>
              <Rich text={item.feedback} />
            </p>
          </div>
        )}
        {answering && (
          <form
            className="card-reply"
            onSubmit={(e) => {
              e.preventDefault();
              answer(item);
            }}
          >
            <textarea
              value={replying === item.id || replying === null ? reply : ""}
              onChange={(e) => {
                setReplying(item.id);
                setReply(e.target.value);
              }}
              placeholder={
                question
                  ? "Tu respuesta. El agente vuelve a empezar con ella."
                  : "Qué quieres que cambie o tenga en cuenta en el próximo intento."
              }
              rows={2}
              aria-label={`Responder al agente en la mejora #${item.id}`}
            />
            <div className="card-actions">
              {replying === item.id && !question && (
                <button type="button" className="btn" onClick={() => setReplying(null)}>
                  Cancelar
                </button>
              )}
              <button className="btn primary" disabled={!(replying === item.id && reply.trim())}>
                Responder y seguir
              </button>
            </div>
          </form>
        )}
        {(item.result || item.detail.length > 160) && (
          <button className="link" onClick={() => setExpanded(open ? null : item.id)}>
            {open ? "Ver menos" : "Ver todo"}
          </button>
        )}

        <div className="card-foot">
          <span className="stamp">{ago(item.updated_at)}</span>
          <div className="card-actions">
            {actions(item)}
            {!running && (
              <button className="icon-btn" title="Borrar" onClick={() => act(() => api.deleteImprovement(item.id))}>
                ×
              </button>
            )}
          </div>
        </div>
      </article>
    );
  };

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Mejoras</h1>
          <p>
            Al aceptar una mejora, Claude la resuelve completa por su cuenta en una rama aparte y te cuenta la solución. Nada se
            sube a producción hasta que tú la fusionas.
          </p>
        </div>
        <button className="btn primary" onClick={() => setAdding((v) => !v)} disabled={!contexts.length}>
          {adding ? "Cancelar" : "+ Manual"}
        </button>
      </header>

      {error && <div className="banner">{error}</div>}

      {adding && (
        <form className="panel form" onSubmit={add}>
          <div className="form-row">
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
            <label className="grow">
              Título
              <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Qué quieres mejorar" autoFocus />
            </label>
          </div>
          <label>
            Detalle
            <textarea
              value={detail}
              onChange={(e) => setDetail(e.target.value)}
              placeholder="Qué problema resuelve y cómo te imaginas que quede. Mientras más claro, mejor trabaja el agente."
              rows={3}
            />
          </label>
          <div className="form-actions">
            <button className="btn primary" disabled={!title.trim()}>
              Guardar propuesta
            </button>
          </div>
        </form>
      )}

      <div className="board four">
        {COLUMNS.map((column) => {
          const items = column.states.flatMap((state) => improvements.filter((i) => i.status === state));
          return (
            <section className="column" key={column.title}>
              <header>
                <span className="column-title">
                  <Pip status={column.tone} /> {column.title} <span className="count">{items.length}</span>
                </span>
                <span className="stamp">{column.note}</span>
              </header>
              {items.length ? items.map(card) : <Empty>Nada aquí.</Empty>}
            </section>
          );
        })}
      </div>

      {discarded.length > 0 && (
        <div className="stack">
          <button className="link" onClick={() => setShowDiscarded((v) => !v)}>
            {showDiscarded ? "Ocultar" : "Ver"} descartadas ({discarded.length})
          </button>
          {showDiscarded && <div className="board four">{discarded.map(card)}</div>}
        </div>
      )}
    </div>
  );
}
