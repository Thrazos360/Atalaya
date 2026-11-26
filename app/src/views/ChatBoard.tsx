import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { type Improvement, type ImprovementState, api } from "../api";

/**
 * Los controles de Mejoras y Decisiones dentro del chat: la barra de lo pendiente y las
 * tarjetas que aparecen bajo cada paso que toca una mejora o una decisión. Hacen lo mismo
 * que los botones de esas vistas, para no tener que salir de la conversación.
 */

export type Decision = {
  id: number;
  slug: string;
  kind: string;
  title: string;
  context: string;
  proposal: string;
  action: "mejora" | "comentario" | "reconocer" | "ninguna";
  target: string | null;
  status: "pendiente" | "aprobada" | "descartada";
  result: string | null;
  created_at: string;
};

/** Algo que una IA propuso recordar (ver `memories.rs`). */
export type Memory = {
  id: number;
  slug: string | null;
  text: string;
  status: "propuesta" | "aprobada" | "descartada";
  source: string;
  created_at: string;
};

/** Una mejora, una decisión o una memoria a la que se refiere un paso del chat. */
export type ItemRef = { kind: "mejora" | "decision" | "memoria"; id: number };

/** Lo que el chat hace por estos controles: escribir por ti o pasarle algo a Claude Code. */
export type BoardHandlers = {
  /** Deja el comienzo de un mensaje en la caja, para que lo completes. */
  draft: (text: string) => void;
  /** Le pide a Claude Code que trabaje la mejora en esta conversación. */
  workHere: (item: Improvement) => void;
  /** Responder la pregunta que dejó el agente de Mejoras. */
  answer: (item: Improvement) => void;
  /** Vuelve a leer mejoras y decisiones después de un cambio. */
  changed: () => void;
};

const STATE_LABEL: Record<ImprovementState, string> = {
  propuesta: "propuesta",
  aceptada: "aceptada",
  en_curso: "en curso",
  en_revision: "en revisión",
  hecha: "hecha",
  descartada: "descartada",
};

const ACTION_LABEL: Record<Decision["action"], string> = {
  mejora: "crea una mejora",
  comentario: "comenta en el issue",
  reconocer: "reconoce la alerta",
  ninguna: "solo informa",
};

/** Lo que el agente de Mejoras preguntó, si quedó esperando tu respuesta. */
export function questionOf(item: Improvement): string | null {
  if (item.status !== "aceptada" || !item.result) return null;
  const lines = item.result.split("\n").map((l) => l.trim());
  const marked = lines.find((l) => /^[*_]*pregunta:/i.test(l));
  if (marked) return marked.replace(/^[*_]*pregunta:[*_]*\s*/i, "");
  const last = lines.filter(Boolean).pop() ?? "";
  return last.endsWith("?") ? last : null;
}

/** Las decisiones, al día: al entrar a la vista, al terminar un turno y cada 30 segundos. */
export function useDecisions(visible: boolean, busy: boolean): [Decision[], () => void] {
  const [decisions, setDecisions] = useState<Decision[]>([]);
  const load = () =>
    invoke<{ decisions: Decision[] }>("list_decisions")
      .then((d) => setDecisions(d.decisions))
      .catch(() => {});
  useEffect(() => {
    if (!visible) return;
    load();
    const timer = window.setInterval(load, 30_000);
    return () => window.clearInterval(timer);
  }, [visible]);
  useEffect(() => {
    if (!busy && visible) load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy]);
  return [decisions, load];
}

/** La memoria propuesta y la aprobada, al día como las decisiones. */
export function useMemories(visible: boolean, busy: boolean): [Memory[], () => void] {
  const [memories, setMemories] = useState<Memory[]>([]);
  const load = () =>
    invoke<Memory[]>("memories_list")
      .then(setMemories)
      .catch(() => {});
  useEffect(() => {
    if (!visible) return;
    load();
    const timer = window.setInterval(load, 30_000);
    return () => window.clearInterval(timer);
  }, [visible]);
  useEffect(() => {
    if (!busy && visible) load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy]);
  return [memories, load];
}

/** Aprobar (tal cual o ajustada) o descartar una memoria propuesta. */
function MemoryControls({ item, on }: { item: Memory; on: BoardHandlers }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(item.text);
  const decide = async (approve: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await invoke("memories_decide", { id: item.id, approve, text: approve && editing ? text : null });
      on.changed();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  if (item.status !== "propuesta") return null;
  return (
    <>
      {editing ? (
        <textarea className="tablero-edit" rows={2} value={text} onChange={(e) => setText(e.target.value)} />
      ) : (
        <p className="tablero-proposal">{item.text}</p>
      )}
      <div className="tablero-actions">
        <button type="button" className="btn primary" disabled={busy || !text.trim()} onClick={() => decide(true)}>
          {editing ? "Guardar y aprobar" : "Aprobar"}
        </button>
        {!editing && (
          <button type="button" className="btn" disabled={busy} onClick={() => setEditing(true)}>
            Ajustar
          </button>
        )}
        <button type="button" className="btn" disabled={busy} onClick={() => decide(false)}>
          Descartar
        </button>
      </div>
      {error && <p className="tablero-error">{error}</p>}
    </>
  );
}

/** Los botones de una mejora, según su etapa. */
function ImprovementControls({ item, canWork, on }: { item: Improvement; canWork: boolean; on: BoardHandlers }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState(false);

  const act = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await task();
      on.changed();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const move = (status: ImprovementState) => act(() => api.setImprovementStatus(item.id, status));
  const queue = () =>
    act(async () => {
      if (item.status !== "aceptada") await api.setImprovementStatus(item.id, "aceptada");
      await api.startImprovement(item.id);
    });
  const button = (label: string, run: () => void, primary = false, title?: string) => (
    <button key={label} type="button" className={`btn${primary ? " primary" : ""}`} disabled={busy} onClick={run} title={title}>
      {label}
    </button>
  );

  const question = questionOf(item);
  let buttons: React.ReactNode[] = [];
  switch (item.status) {
    case "propuesta":
      buttons = [
        canWork && button("Aceptar y trabajar", queue, true, "La acepta y el agente de Mejoras la empieza"),
        button("Aceptar", () => move("aceptada")),
        button("Hazla aquí", () => on.workHere(item), false, "Claude Code la trabaja en esta conversación"),
        button("Descartar", () => move("descartada")),
      ];
      break;
    case "aceptada":
      buttons = question
        ? [button("Responder", () => on.answer(item), true), button("Descartar", () => move("descartada"))]
        : [
            button("Hazla aquí", () => on.workHere(item), true, "Claude Code la trabaja en esta conversación"),
            canWork &&
              button(
                item.result ? "Reintentar con el agente" : "Mandar al agente",
                queue,
                false,
                "La pone en la cola del agente de Mejoras",
              ),
            button("Marcar hecha", () => move("hecha")),
            button("Descartar", () => move("descartada")),
          ];
      break;
    case "en_revision":
      buttons = [
        item.pr_url && button("Ver PR ↗", () => openUrl(item.pr_url!)),
        item.pr_url &&
          (confirming
            ? button("Confirmar: sale a producción", () => act(() => api.mergeImprovement(item.id)), true)
            : button("Fusionar", () => setConfirming(true), true)),
        !item.pr_url && button("Marcar hecha", () => move("hecha"), true),
        button("Pedir cambios", () => on.draft(`Sobre la mejora #${item.id} «${item.title}», cambia esto: `)),
        button("Descartar", () => move("descartada")),
      ];
      break;
    case "hecha":
      buttons = [button("Reabrir", () => move("aceptada"))];
      break;
    case "descartada":
      buttons = [button("Recuperar", () => move("propuesta"))];
      break;
  }

  return (
    <>
      {question && <p className="card-question">{question}</p>}
      {buttons.some(Boolean) && <div className="tablero-actions">{buttons}</div>}
      {error && <p className="tablero-error">{error}</p>}
    </>
  );
}

/** Los botones de una decisión pendiente. */
function DecisionControls({ item, on }: { item: Decision; on: BoardHandlers }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const act = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await task();
      on.changed();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  if (item.status !== "pendiente") return item.result ? <p className="stamp">{item.result}</p> : null;
  const approve = (work: boolean) => act(() => invoke("approve_decision", { id: item.id, proposal: item.proposal, work }));
  return (
    <>
      {item.proposal && <p className="tablero-proposal">{item.proposal}</p>}
      <div className="tablero-actions">
        <button type="button" className="btn primary" disabled={busy} onClick={() => approve(false)}>
          Aprobar · {ACTION_LABEL[item.action]}
        </button>
        {item.action === "ninguna" && (
          <button
            type="button"
            className="btn"
            disabled={busy}
            onClick={() => approve(true)}
            title="La convierte en mejora y el agente la trabaja"
          >
            Aprobar y resolver
          </button>
        )}
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => on.draft(`Sobre la decisión #${item.id} «${item.title}»: `)}
        >
          Preguntar
        </button>
        <button
          type="button"
          className="btn"
          disabled={busy}
          onClick={() => act(() => invoke("discard_decision", { id: item.id }))}
        >
          Descartar
        </button>
      </div>
      {error && <p className="tablero-error">{error}</p>}
    </>
  );
}

/** Una mejora o decisión con sus botones, bajo el paso del chat que la tocó. */
export function ItemCard({
  target,
  improvements,
  decisions,
  memories = [],
  projectName,
  canWork,
  on,
}: {
  target: ItemRef;
  improvements: Improvement[];
  decisions: Decision[];
  memories?: Memory[];
  projectName: (slug: string) => string;
  canWork: (slug: string) => boolean;
  on: BoardHandlers;
}) {
  if (target.kind === "memoria") {
    const item = memories.find((m) => m.id === target.id);
    if (!item) return null;
    return (
      <div className="tablero-card">
        <div className="tablero-head">
          <span className="tablero-kind">Memoria #{item.id}</span>
          <span className={`tablero-state ${item.status}`}>{item.status}</span>
          <span className="tablero-project">{item.slug ? projectName(item.slug) : "Todos los proyectos"}</span>
        </div>
        {item.status === "propuesta" ? <MemoryControls item={item} on={on} /> : <p className="tablero-proposal">{item.text}</p>}
      </div>
    );
  }
  if (target.kind === "mejora") {
    const item = improvements.find((i) => i.id === target.id);
    if (!item) return null;
    return (
      <div className="tablero-card">
        <div className="tablero-head">
          <span className="tablero-kind">Mejora #{item.id}</span>
          <span className={`tablero-state ${item.status}`}>{STATE_LABEL[item.status]}</span>
          <span className="tablero-project">{projectName(item.slug)}</span>
        </div>
        <strong className="tablero-title">{item.title}</strong>
        <ImprovementControls item={item} canWork={canWork(item.slug)} on={on} />
      </div>
    );
  }
  const item = decisions.find((d) => d.id === target.id);
  if (!item) return null;
  return (
    <div className="tablero-card">
      <div className="tablero-head">
        <span className="tablero-kind">Decisión #{item.id}</span>
        <span className={`tablero-state ${item.status}`}>{item.status}</span>
        <span className="tablero-project">{projectName(item.slug)}</span>
      </div>
      <strong className="tablero-title">{item.title}</strong>
      <DecisionControls item={item} on={on} />
    </div>
  );
}

type Group = { id: string; label: string; items: ItemRef[]; hot?: boolean };

/** La barra de lo que espera algo de ti, con su lista desplegable. */
export function BoardBar({
  improvements,
  decisions,
  memories = [],
  slug,
  projectName,
  canWork,
  on,
}: {
  improvements: Improvement[];
  decisions: Decision[];
  memories?: Memory[];
  /** El proyecto en foco: si hay uno, solo lo suyo. */
  slug: string;
  projectName: (slug: string) => string;
  canWork: (slug: string) => boolean;
  on: BoardHandlers;
}) {
  const [open, setOpen] = useState<string | null>(null);
  const mine = (s: string) => !slug || s === slug;
  const imps = improvements.filter((i) => mine(i.slug));
  const ref = (kind: ItemRef["kind"]) => (x: { id: number }) => ({ kind, id: x.id });
  const groups: Group[] = [
    {
      id: "decisiones",
      label: "Decisiones",
      items: decisions.filter((d) => d.status === "pendiente" && mine(d.slug)).map(ref("decision")),
      hot: true,
    },
    {
      id: "memoria",
      label: "Memoria",
      items: memories.filter((m) => m.status === "propuesta" && (!m.slug || mine(m.slug))).map(ref("memoria")),
      hot: true,
    },
    { id: "preguntan", label: "Te preguntan", items: imps.filter((i) => questionOf(i)).map(ref("mejora")), hot: true },
    { id: "revision", label: "En revisión", items: imps.filter((i) => i.status === "en_revision").map(ref("mejora")) },
    { id: "propuestas", label: "Propuestas", items: imps.filter((i) => i.status === "propuesta").map(ref("mejora")) },
    {
      id: "aceptadas",
      label: "Aceptadas",
      items: imps.filter((i) => i.status === "aceptada" && !questionOf(i)).map(ref("mejora")),
    },
    { id: "curso", label: "En curso", items: imps.filter((i) => i.status === "en_curso").map(ref("mejora")) },
  ].filter((g) => g.items.length > 0);
  if (!groups.length) return null;
  const shown = groups.find((g) => g.id === open);

  return (
    <div className="tablero">
      <div className="tablero-bar" role="toolbar" aria-label="Mejoras y decisiones">
        <span className="chat-team-title">Tablero</span>
        {groups.map((g) => (
          <button
            key={g.id}
            type="button"
            className={`tablero-chip${g.hot ? " hot" : ""}${open === g.id ? " open" : ""}`}
            aria-expanded={open === g.id}
            onClick={() => setOpen(open === g.id ? null : g.id)}
          >
            {g.label} <strong>{g.items.length}</strong>
          </button>
        ))}
      </div>
      {shown && (
        <div className="tablero-list">
          {shown.items.map((t) => (
            <ItemCard
              key={`${t.kind}-${t.id}`}
              target={t}
              improvements={improvements}
              decisions={decisions}
              memories={memories}
              projectName={projectName}
              canWork={canWork}
              on={on}
            />
          ))}
        </div>
      )}
    </div>
  );
}
