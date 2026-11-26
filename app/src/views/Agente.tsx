import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Console } from "../App";
import { type Improvement, api } from "../api";
import { Rich, Tag } from "../ui";
import { BoardBar, type BoardHandlers, ItemCard, type ItemRef, useDecisions, useMemories } from "./ChatBoard";
import { type Attachment, FileStrip, useAttachments } from "./ChatFiles";
import { ChatHistory, type SavedConversation, type SavedMessage } from "./ChatHistory";

type Entry =
  | { role: "user" | "error"; text: string; files?: Attachment[] }
  // `who`: qué IA respondió, para saber si de verdad se reparten el trabajo.
  | { role: "assistant"; text: string; who?: string }
  | Tool
  | { role: "output"; text: string; failed: boolean; parent?: string | null }
  // La misma pregunta a varias IA a la vez: cada respuesta llega cuando está.
  | { role: "compare"; request: string; question: string; names: string[]; answers: (CompareAnswer | null)[] };

/** Una respuesta de una comparación (`atalaya_core::compare::Answer`). */
type CompareAnswer = {
  id: string;
  name: string;
  ok: boolean;
  text: string;
  seconds: number;
  cost_usd: number | null;
};

/** La conversación como se guarda en el historial: solo el texto, uno a uno con lo que se ve. */
function toSaved(entries: Entry[], main: string): SavedMessage[] {
  return entries.flatMap((e): SavedMessage[] => {
    switch (e.role) {
      case "user":
        return [{ role: "usuario", author: "", text: e.text }];
      case "assistant":
        return [{ role: "ia", author: e.who ?? main, text: e.text }];
      case "error":
        return [{ role: "nota", author: "", text: e.text }];
      case "compare":
        return e.names.map((name, i) => {
          const a = e.answers[i];
          return {
            role: "ia",
            author: a?.name ?? name,
            text: !a ? "(sin respuesta)" : a.ok ? a.text : `(no respondió) ${a.text}`,
          };
        });
      default:
        return [];
    }
  });
}

/** Cuántas IA se comparan a la vez. */
const COMPARE_MAX = 4;

/** Lo que escribiste mientras trabajaba, con sus adjuntos. */
type Queued = { text: string; files: Attachment[] };

/** Un paso: quién lo dio (Claude Code o un subagente), bajo qué llamada y si ya terminó. */
type Tool = {
  role: "tool";
  text: string;
  detail: string | null;
  id?: string | null;
  parent?: string | null;
  actor?: string;
  spawns?: string;
  done?: boolean;
  failed?: boolean;
  /** La mejora o decisión que toca este paso: bajo él van sus botones. */
  target?: ItemRef;
};

/** A qué mejora o decisión se refiere un paso, por su detalle («#12 → hecha»). */
function targetOf(tool: string, detail: string | null): ItemRef | undefined {
  const id = Number(detail?.match(/^#(\d+)/)?.[1]);
  if (!id) return undefined;
  if (tool === "set_improvement_status") return { kind: "mejora", id };
  if (tool === "resolve_decision") return { kind: "decision", id };
  return undefined;
}

/** Las herramientas que crean una mejora o decisión: su número llega en el resultado. */
const CREATES: Record<string, ItemRef["kind"]> = {
  add_improvement: "mejora",
  propose_decision: "decision",
  proponer_memoria: "memoria",
};

type Step = { id: string | null; parent: string | null; actor: string; spawns?: string };

/** Lo que hace una IA a la que Claude Code le encargó algo, en vivo. */
type Run = {
  run: string;
  actor: string;
  what: string;
  lines: string[];
  status: "trabajando" | "listo" | "error";
  end?: string;
  /** La llamada a delegar que lo lanzó. */
  tool?: string | null;
};

type ActivityRow = { id: number; run: string; actor: string; kind: "inicio" | "linea" | "fin" | "error"; text: string };

const DELEGATE_TOOLS = ["delegar_consulta", "delegar_trabajo", "delegar_consultas", "delegar_trabajos"];
const CLAUDE = "Claude Code";

type Option = { id: string; name: string; kind: "claude-code" | "agente" | "ia" };
type RoleSummary = { roles: { id: string; candidates: { id: string; name: string; ready: boolean }[]; options: Option[] }[] };

type ChatEvent = {
  request: string;
  kind: "quien" | "delta" | "texto" | "herramienta" | "resultado" | "fin" | "error";
  text: string;
  detail: string | null;
  session: string | null;
  step?: Step;
};

type Approval = {
  id: number;
  conversation: string;
  tool: string;
  summary: string;
  detail: string;
  created_at: string;
  /** El paso que lo pide: con él se sabe si es Claude Code o uno de sus subagentes. */
  step: string | null;
};

type Mode = "preguntar" | "autonomo" | "lectura";

const MODES: { id: Mode; label: string; note: string }[] = [
  {
    id: "preguntar",
    label: "Puede todo, pregunta antes de cambiar",
    note: "Lee, edita, ejecuta órdenes y usa git y las herramientas de Atalaya. Cada cambio espera tu visto bueno aquí.",
  },
  {
    id: "autonomo",
    label: "Puede todo sin preguntar",
    note: "Hace los cambios sin esperar tu visto bueno. Antes de algo irreversible o que toque producción, te pregunta en la respuesta.",
  },
  { id: "lectura", label: "Solo lectura", note: "Solo lee el código y el estado. Puede anotar mejoras." },
];

/** Los agentes de terminal no saben pedir permiso desde la app: o pueden todo, o solo leen. */
const AGENT_MODES: { id: Mode; label: string; note: string }[] = [
  {
    id: "autonomo",
    label: "Puede todo en tus proyectos",
    note: "Lee, edita y ejecuta órdenes en las carpetas de tus proyectos, sin pedir tu visto bueno. Fuera de ellas no puede escribir, y no ve tus llaves.",
  },
  {
    id: "lectura",
    label: "Solo lectura",
    note: "Lee el código y el estado de tus proyectos, sin cambiar nada. Para que haga cambios, elige «Puede todo en tus proyectos».",
  },
];

// Claude Code y los demás agentes recuerdan su modo por separado: dar libertad a uno no se la da al otro.
const MODE_KEY = "atalaya.agente.modo";
const AGENT_MODE_KEY = "atalaya.agente.modo-cli";

function savedMode(key: string, fallback: Mode): Mode {
  try {
    const m = localStorage.getItem(key);
    if (m === "preguntar" || m === "autonomo" || m === "lectura") return m;
  } catch {
    // Sin almacenamiento: el modo normal.
  }
  return fallback;
}

const TOOL_LABEL: Record<string, string> = {
  list_projects: "Lista los proyectos",
  get_project_context: "Lee el contexto del proyecto",
  recent_commits: "Revisa los últimos cambios",
  project_status: "Consulta la vigilancia",
  list_flags: "Lee los interruptores",
  set_flag: "Cambia un interruptor",
  list_improvements: "Lee las mejoras",
  add_improvement: "Anota una mejora",
  set_improvement_status: "Mueve una mejora de etapa",
  list_decisions: "Lee las decisiones",
  propose_decision: "Propone una decisión",
  resolve_decision: "Resuelve una decisión",
  delegar_consulta: "Le pregunta a otra IA",
  delegar_trabajo: "Le encarga un cambio a otra IA",
  delegar_consultas: "Reparte consultas entre varias IA a la vez",
  delegar_trabajos: "Reparte cambios entre varios agentes a la vez",
  Read: "Lee",
  Write: "Escribe",
  Edit: "Edita",
  MultiEdit: "Edita",
  Bash: "Ejecuta",
  Grep: "Busca",
  Glob: "Busca archivos",
  WebFetch: "Lee la web",
  WebSearch: "Busca en la web",
  TodoWrite: "Ordena sus pasos",
  ToolSearch: "Busca herramientas",
  Skill: "Usa la skill",
  proponer_memoria: "Propone recordar",
  list_memories: "Lee la memoria",
  Task: "Lanza un subagente",
  Agent: "Lanza un subagente",
};

/** El equipo de esta conversación: quién trabajó, quién sigue trabajando y en qué. */
type Member = { name: string; kind: "principal" | "subagente" | "delegada"; working: boolean; failed?: boolean; doing?: string };

function teamOf(entries: Entry[], runs: Run[], busy: boolean, main: string): Member[] {
  const team: Member[] = [];
  const tools = entries.filter((e): e is Tool => e.role === "tool");
  if (!tools.some((t) => t.actor) && !runs.length) return team;
  const last = [...tools].reverse().find((t) => !t.actor || t.actor === CLAUDE);
  team.push({ name: main, kind: "principal", working: busy, doing: last ? (TOOL_LABEL[last.text] ?? last.text) : undefined });
  for (const t of tools) {
    if (!t.spawns) continue;
    const steps = tools.filter((s) => s.parent && s.parent === t.id);
    const now = steps[steps.length - 1];
    team.push({
      name: t.spawns,
      kind: "subagente",
      working: busy && !t.done,
      failed: t.failed,
      doing: now ? `${TOOL_LABEL[now.text] ?? now.text}${now.detail ? ` ${now.detail}` : ""}` : undefined,
    });
  }
  for (const r of runs) {
    team.push({
      name: r.actor,
      kind: "delegada",
      working: r.status === "trabajando",
      failed: r.status === "error",
      doing: r.status === "trabajando" ? (r.lines[r.lines.length - 1] ?? r.what) : r.what,
    });
  }
  return team;
}

/** La franja de arriba: quién está trabajando ahora y quién ya terminó. */
function Team({ members }: { members: Member[] }) {
  if (members.length < 2) return null;
  return (
    <div className="chat-team" aria-label="Equipo de esta conversación">
      <span className="chat-team-title">Equipo</span>
      {members.map((m, i) => (
        <span
          key={i}
          className={`chat-member ${m.kind}${m.working ? " working" : ""}${m.failed ? " failed" : ""}`}
          title={m.doing}
        >
          <span className="dot" />
          <strong>{m.name}</strong>
          <span className="state">{m.working ? "trabajando" : m.failed ? "falló" : "listo"}</span>
        </span>
      ))}
    </div>
  );
}

/** Lo que va haciendo una IA delegada, bajo la llamada que la lanzó. */
function LiveRun({ run }: { run: Run }) {
  const [open, setOpen] = useState(false);
  const shown = open ? run.lines : run.lines.slice(-5);
  return (
    <div className={`chat-live ${run.status}`}>
      <div className="chat-live-head">
        <span className="chat-actor delegada">{run.actor}</span>
        {run.status === "trabajando" ? "trabajando…" : run.status === "error" ? "no pudo terminar" : "terminó"}
        {run.lines.length > 5 && (
          <button type="button" className="link" onClick={() => setOpen(!open)}>
            {open ? "Ver menos" : `Ver todo (${run.lines.length} líneas)`}
          </button>
        )}
      </div>
      {shown.length > 0 && <pre className="chat-live-lines">{shown.join("\n")}</pre>}
      {run.end && (run.status === "error" || (run.status === "listo" && !run.lines.length)) && (
        <div className="chat-live-end">{run.end}</div>
      )}
    </div>
  );
}

const SUGGESTIONS = [
  "¿Qué requiere mi atención hoy?",
  "Revisa por qué falla el build del proyecto y arréglalo",
  "Actualiza las dependencias con parches de seguridad y corre las pruebas",
];

/** Una acción que espera tu visto bueno. */
function ApprovalCard({ item, who, onAnswered }: { item: Approval; who?: string; onAnswered: () => void }) {
  const [why, setWhy] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const answer = async (allow: boolean, remember: boolean) => {
    setBusy(true);
    try {
      await invoke("chat_answer", { id: item.id, allow, remember, message: allow ? null : why.trim() || null });
      onAnswered();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div className="chat-approval">
      <div className="chat-who">Espera tu visto bueno{who ? ` · lo pide ${who}` : ""}</div>
      <div className="chat-approval-title">{item.summary}</div>
      {item.detail && <pre className="chat-pre">{item.detail}</pre>}
      <div className="chat-approval-actions">
        <button className="btn primary" disabled={busy} onClick={() => answer(true, false)}>
          Permitir
        </button>
        <button
          className="btn"
          disabled={busy}
          onClick={() => answer(true, true)}
          title={`No vuelve a preguntar por ${item.tool} en esta conversación`}
        >
          Permitir siempre en esta conversación
        </button>
        <input
          value={why}
          onChange={(e) => setWhy(e.target.value)}
          placeholder="Por qué no, o qué prefieres (opcional)"
          disabled={busy}
        />
        <button className="btn" disabled={busy} onClick={() => answer(false, false)}>
          Rechazar
        </button>
      </div>
      {error && <div className="error-note">{error}</div>}
    </div>
  );
}

/** La salida de una herramienta, plegada salvo que sea corta. */
function Output({ text, failed }: { text: string; failed: boolean }) {
  const lines = text.split("\n");
  if (lines.length <= 3 && text.length <= 240) {
    return <pre className={`chat-pre chat-output${failed ? " failed" : ""}`}>{text}</pre>;
  }
  return (
    <details className={`chat-output${failed ? " failed" : ""}`}>
      <summary>
        {failed ? "Falló · " : ""}
        {lines.length === 1 ? "1 línea" : `${lines.length} líneas`} de salida
      </summary>
      <pre className="chat-pre">{text}</pre>
    </details>
  );
}

/** Lo que App muestra en la barra mientras el Agente trabaja en otra vista. */
export type AgentActivity = { busy: boolean; waiting: number };

/**
 * Una terminal de IA: Claude Code u otro agente con sus herramientas, o un modelo por API para conversar.
 * App la deja montada aunque cambies de vista: la conversación y el trabajo siguen.
 */
/** Lo que el agente de Mejoras preguntó o dijo al quedarse sin poder terminar. */
function askedOf(item: Improvement): string {
  const lines = (item.result ?? "").split("\n").map((l) => l.trim());
  const marked = lines.find((l) => /^[*_]*pregunta:/i.test(l));
  if (marked) return marked.replace(/^[*_]*pregunta:[*_]*\s*/i, "");
  return lines.filter(Boolean).pop() ?? "No pudo terminarla.";
}

/** El encargo para Claude Code: retomar la mejora con la respuesta del usuario. */
function takeOver(item: Improvement, project: string, answer: string): string {
  return [
    `Retoma la mejora #${item.id} del proyecto «${project}» (slug \`${item.slug}\`): «${item.title}».`,
    `Detalle: ${item.detail || "(sin detalle)"}`,
    `El agente de Mejoras no pudo terminarla y dejó esto:\n${item.result ?? "(sin informe)"}`,
    item.feedback ? `Lo que ya le respondí antes:\n${item.feedback}` : "",
    `Mi respuesta: ${answer}`,
    "Hazla completa con mi respuesta, sin volver a preguntar lo que ya respondí, y llévala por sus etapas en el tablero (en curso, en revisión con la rama o el PR, hecha).",
  ]
    .filter(Boolean)
    .join("\n\n");
}

export function Agente({
  contexts,
  improvements,
  refresh,
  visible,
  onActivity,
  handoff,
  onHandoffTaken,
}: Console & {
  visible: boolean;
  onActivity: (activity: AgentActivity) => void;
  /** Una mejora que se quedó sin terminar y espera tu respuesta. */
  handoff: Improvement | null;
  onHandoffTaken: () => void;
}) {
  const [entries, setEntries] = useState<Entry[]>([]);
  // Lo que escribes mientras trabaja: sale cuando termina o cuando lo detienes.
  const [queue, setQueue] = useState<Queued[]>([]);
  const [draft, setDraft] = useState("");
  const [slug, setSlug] = useState("");
  const [busy, setBusy] = useState(false);
  const [model, setModel] = useState("");
  const [models, setModels] = useState<Option[]>([]);
  const [usual, setUsual] = useState("Claude Code");
  const [usualKind, setUsualKind] = useState<Option["kind"]>("claude-code");
  const [mode, setMode] = useState<Mode>(() => savedMode(MODE_KEY, "preguntar"));
  const [agentMode, setAgentMode] = useState<Mode>(() => savedMode(AGENT_MODE_KEY, "lectura"));
  const [pending, setPending] = useState<Approval[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  // Comparar: `null` apagado; si no, las IA elegidas.
  const [comparing, setComparing] = useState<string[] | null>(null);
  const [showHistory, setShowHistory] = useState(false);
  const lastSaved = useRef("");
  // Al seguir con una de las que compararon, lo que preguntaste y lo que respondió va con tu próximo mensaje.
  const carry = useRef<string | null>(null);
  const [decisions, reloadDecisions] = useDecisions(visible, busy);
  const [memories, reloadMemories] = useMemories(visible, busy);
  // Los comandos guardados: `/nombre` en la caja los sugiere; al enviar, Atalaya arma el mensaje.
  const [commands, setCommands] = useState<{ name: string; description: string }[]>([]);
  useEffect(() => {
    if (visible)
      invoke<{ name: string; description: string }[]>("commands_list")
        .then(setCommands)
        .catch(() => {});
  }, [visible]);
  const typed = draft.match(/^\/([\w-]*)$/)?.[1];
  const suggestions = typed === undefined ? [] : commands.filter((c) => c.name.startsWith(typed.toLowerCase())).slice(0, 6);
  const seen = useRef(0);
  const entriesRef = useRef<Entry[]>([]);
  entriesRef.current = entries;
  // La mejora que espera tu respuesta: lo próximo que escribas va para ella.
  const [asking, setAsking] = useState<Improvement | null>(null);
  const session = useRef<string | null>(null);
  const who = useRef<string | undefined>(undefined);
  const request = useRef("");
  const conversation = useRef<string>(crypto.randomUUID());
  const attach = useAttachments(conversation, visible);
  const end = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  // El oyente se registra una sola vez; lo que cambia en cada render lo lee de aquí.
  const refreshRef = useRef(refresh);
  refreshRef.current = refresh;
  const finished = useRef(() => {});
  const boardChanged = useRef(() => {});

  useEffect(() => {
    const stop = listen<{ request: string; index: number; answer: CompareAnswer }>("comparar", ({ payload }) => {
      setEntries((all) =>
        all.map((e) =>
          e.role === "compare" && e.request === payload.request
            ? { ...e, answers: e.answers.map((a, j) => (j === payload.index ? payload.answer : a)) }
            : e,
        ),
      );
    });
    return () => {
      stop.then((f) => f());
    };
  }, []);

  useEffect(() => {
    const stop = listen<ChatEvent>("chat", ({ payload }) => {
      if (payload.request !== request.current) return;
      if (payload.session) session.current = payload.session;
      switch (payload.kind) {
        case "quien":
          who.current = payload.text;
          break;
        case "delta":
          setEntries((all) => {
            const last = all[all.length - 1];
            if (last?.role === "assistant") return [...all.slice(0, -1), { ...last, text: last.text + payload.text }];
            const text = payload.text.replace(/^\s+/, "");
            return text ? [...all, { role: "assistant", text, who: who.current }] : all;
          });
          break;
        case "texto":
          setEntries((all) => [...all, { role: "assistant", text: payload.text, who: who.current }]);
          break;
        case "herramienta": {
          const step = payload.step;
          setEntries((all) => [
            ...all,
            {
              role: "tool",
              text: payload.text,
              detail: payload.detail,
              id: step?.id,
              parent: step?.parent,
              actor: step?.actor,
              spawns: step?.spawns,
              target: targetOf(payload.text, payload.detail),
            },
          ]);
          break;
        }
        case "resultado": {
          const step = payload.step;
          const failed = payload.detail === "error";
          // Si el paso tocó una mejora o decisión, sus tarjetas se ponen al día enseguida.
          const touched = entriesRef.current.find((e): e is Tool => e.role === "tool" && !!step?.id && e.id === step.id);
          if (touched && !failed && (touched.target || CREATES[touched.text])) boardChanged.current();
          setEntries((all) => {
            // El paso con el mismo id ya terminó.
            const marked = step?.id
              ? all.map((e) => {
                  if (e.role !== "tool" || e.id !== step.id) return e;
                  // Lo que se acaba de crear trae su número en el resultado: {"id": 12}.
                  const kind = CREATES[e.text];
                  const id = kind && !failed ? Number(payload.text.match(/"id"\s*:\s*(\d+)/)?.[1]) : 0;
                  return { ...e, done: true, failed, target: id ? { kind, id } : e.target };
                })
              : all;
            return payload.text ? [...marked, { role: "output", text: payload.text, failed, parent: step?.parent }] : marked;
          });
          break;
        }
        case "error":
          setEntries((all) => [...all, { role: "error", text: payload.text }]);
          break;
      }
      if (payload.kind === "fin" || payload.kind === "error") {
        setBusy(false);
        setPending([]);
        // El agente pudo cambiar interruptores, mejoras o proyectos: se refresca la consola.
        refreshRef.current();
        finished.current();
      }
    });
    return () => {
      stop.then((off) => off());
    };
  }, []);

  useEffect(() => onActivity({ busy, waiting: pending.length }), [busy, pending.length, onActivity]);

  // Al volver a la vista: el cursor en la caja y lo último a la vista.
  useEffect(() => {
    if (visible) input.current?.focus();
  }, [visible]);

  // Quién puede responder: el asistente de Ajustes → IA, o cualquier otro modelo por API.
  // Se vuelve a leer al entrar, por si cambiaste los roles en otra vista.
  useEffect(() => {
    if (!visible) return;
    invoke<RoleSummary>("ai_roles")
      .then((s) => {
        const role = s.roles.find((r) => r.id === "asistente");
        if (!role) return;
        setModels(role.options);
        const first = role.candidates.find((c) => c.ready);
        if (first) {
          setUsual(first.name);
          setUsualKind(role.options.find((o) => o.id === first.id)?.kind ?? (first.id === "claude-code" ? "claude-code" : "ia"));
        }
      })
      .catch(() => {});
  }, [visible]);

  // Lo nuevo de las IA delegadas: cada encargo queda bajo la llamada a delegar que lo lanzó.
  // Una lectura a la vez: dos cruzadas repetirían las mismas líneas.
  const pulling = useRef(false);
  const pullActivity = () => {
    if (pulling.current) return;
    pulling.current = true;
    const asked = conversation.current;
    invoke<ActivityRow[]>("chat_activity", { conversation: asked, after: seen.current })
      .then((rows) => {
        // Si empezaste otra conversación mientras tanto, esto ya no va.
        if (!rows.length || asked !== conversation.current) return;
        seen.current = rows[rows.length - 1].id;
        setRuns((all) => {
          const next = all.map((r) => ({ ...r, lines: [...r.lines] }));
          for (const row of rows) {
            let run = next.find((r) => r.run === row.run);
            if (!run) {
              const calls = entriesRef.current.filter((e): e is Tool => e.role === "tool" && DELEGATE_TOOLS.includes(e.text));
              const open = calls.filter((c) => !c.done);
              const tool = (open[0] ?? calls[calls.length - 1])?.id ?? null;
              run = { run: row.run, actor: row.actor, what: "", lines: [], status: "trabajando", tool };
              next.push(run);
            }
            if (row.kind === "inicio") run.what = row.text;
            else if (row.kind === "linea") run.lines.push(row.text);
            else {
              run.status = row.kind === "fin" ? "listo" : "error";
              run.end = row.text;
            }
          }
          return next;
        });
      })
      .catch(() => {})
      .finally(() => {
        pulling.current = false;
      });
  };

  // Al terminar el turno, lo último que dejaron.
  useEffect(() => {
    if (!busy && request.current) {
      pullActivity();
      // Por si justo había otra lectura en camino.
      window.setTimeout(pullActivity, 900);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy]);

  // Mientras trabaja, lo que espera tu visto bueno y lo que hacen las IA delegadas.
  useEffect(() => {
    if (!busy) return;
    let alive = true;
    const check = () => {
      invoke<Approval[]>("chat_pending", { conversation: conversation.current })
        .then((list) => alive && setPending(list))
        .catch(() => {});
      if (alive) pullActivity();
    };
    check();
    const timer = window.setInterval(check, 700);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [busy]);

  useEffect(() => {
    if (visible) end.current?.scrollIntoView({ block: "end" });
  }, [entries, busy, pending.length, queue.length, visible]);

  const kind = model ? (models.find((m) => m.id === model)?.kind ?? "ia") : usualKind;
  const isClaude = kind === "claude-code";
  const isAgent = kind === "agente";
  const modes = isAgent ? AGENT_MODES : MODES;
  // Un agente de terminal no pregunta: empieza en solo lectura.
  const effective: Mode = isAgent ? (agentMode === "autonomo" ? "autonomo" : "lectura") : mode;
  const current = modes.find((m) => m.id === effective) ?? modes[0];

  function chooseMode(next: Mode) {
    (isAgent ? setAgentMode : setMode)(next);
    try {
      localStorage.setItem(isAgent ? AGENT_MODE_KEY : MODE_KEY, next);
    } catch {
      // Sin almacenamiento: vale solo mientras la app está abierta.
    }
  }

  // Al llegar una mejora que espera tu respuesta, se enfoca su proyecto. Si no hay nada en
  // marcha, empieza una conversación nueva para ella.
  useEffect(() => {
    if (!handoff) return;
    if (!busy) {
      session.current = null;
      conversation.current = crypto.randomUUID();
      setEntries([]);
      setPending([]);
      setRuns([]);
      seen.current = 0;
    }
    setAsking(handoff);
    setSlug(handoff.slug);
    onHandoffTaken();
    input.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [handoff]);

  function send(text: string) {
    const files = attach.files;
    let message = text.trim() || (files.length ? "Mira lo que adjunté." : "");
    if (!message) return;
    setDraft("");
    attach.clear();
    const texts = queue.map((q) => q.text);
    const allFiles = [...queue.flatMap((q) => q.files), ...files];
    let shown = message;
    // La respuesta a una mejora queda anotada en ella y Claude Code la retoma con todo el contexto.
    if (asking) {
      const item = asking;
      const name = contexts.find((c) => c.project.slug === item.slug)?.project.name ?? item.slug;
      setAsking(null);
      api.answerImprovement(item.id, message).catch(() => {});
      shown = `Sobre la mejora #${item.id} «${item.title}»: ${message}`;
      message = takeOver(item, name, message);
      if (!busy) {
        dispatch([...texts, message].join("\n\n"), [...texts, shown].join("\n\n"), allFiles);
        setQueue([]);
        return;
      }
    }
    if (comparing) {
      if (busy || comparing.length < 2) return;
      compare(message, comparing);
      return;
    }
    if (busy) {
      setQueue((all) => [...all, { text: message, files }]);
      return;
    }
    // Si quedó algo en cola (por ejemplo, porque falló el envío anterior), sale junto.
    dispatch([...texts, message].join("\n\n"), undefined, allFiles);
    setQueue([]);
  }

  // Al terminar o detenerse, sale lo que escribiste mientras trabajaba, todo en un mensaje.
  finished.current = () => {
    if (!queue.length) return;
    dispatch(
      queue.map((q) => q.text).join("\n\n"),
      undefined,
      queue.flatMap((q) => q.files),
    );
    setQueue([]);
  };

  /** `shown` es lo que se ve en la conversación, si no es el mensaje completo. */
  async function dispatch(message: string, shown = message, files: Attachment[] = []) {
    setEntries((all) => [...all, { role: "user", text: shown, files }]);
    setBusy(true);
    request.current = crypto.randomUUID();
    const full = carry.current ? `${carry.current}\n\n${message}` : message;
    carry.current = null;
    try {
      await invoke("chat_send", {
        request: request.current,
        message: full,
        session: session.current,
        slug: slug || null,
        model: model || null,
        mode: effective,
        conversation: conversation.current,
        attachments: files.map((f) => f.path),
      });
    } catch (e) {
      setEntries((all) => [...all, { role: "error", text: String(e) }]);
      setBusy(false);
    }
  }

  /** La misma pregunta a varias IA a la vez; solo leen. */
  async function compare(question: string, ids: string[]) {
    const id = crypto.randomUUID();
    const names = ids.map((x) => models.find((m) => m.id === x)?.name ?? x);
    setEntries((all) => [
      ...all,
      { role: "user", text: question },
      { role: "compare", request: id, question, names, answers: ids.map(() => null) },
    ]);
    setBusy(true);
    try {
      const answers = await invoke<CompareAnswer[]>("chat_compare", { request: id, question, ais: ids, slug: slug || null });
      setEntries((all) => all.map((e) => (e.role === "compare" && e.request === id ? { ...e, answers } : e)));
    } catch (e) {
      setEntries((all) => [...all, { role: "error", text: String(e) }]);
    } finally {
      setBusy(false);
    }
  }

  /** Sigue la conversación con una de las que compararon, con lo que ya respondió. */
  function continueWith(question: string, answer: CompareAnswer) {
    setComparing(null);
    setModel(answer.id);
    session.current = null;
    carry.current = `Antes te pregunté: «${question}». Respondiste:\n\n${answer.text}\n\nSigamos desde ahí.`;
    setEntries((all) => [
      ...all,
      { role: "assistant", text: `Sigues con ${answer.name}: tu próximo mensaje le llega con su respuesta.` },
    ]);
    input.current?.focus();
  }

  async function stop() {
    try {
      await invoke("chat_stop", { conversation: conversation.current });
    } catch (e) {
      setEntries((all) => [...all, { role: "error", text: String(e) }]);
    }
  }

  function reset() {
    session.current = null;
    request.current = "";
    conversation.current = crypto.randomUUID();
    setEntries([]);
    setQueue([]);
    setPending([]);
    setRuns([]);
    seen.current = 0;
    setBusy(false);
  }

  const main = model ? (models.find((m) => m.id === model)?.name ?? usual) : usual;
  const team = teamOf(entries, runs, busy, main);

  // Tras cada turno la conversación queda en el historial, para buscarla o retomarla después.
  useEffect(() => {
    if (busy || !entries.length) return;
    const messages = toSaved(entries, main);
    const key = `${conversation.current}:${JSON.stringify(messages)}`;
    if (key === lastSaved.current) return;
    lastSaved.current = key;
    invoke("conversations_save", { id: conversation.current, slug: slug || null, model: main, messages }).catch(() => {});
  }, [busy, entries, main, slug]);

  /** Abre una conversación guardada: se ve como estaba y la IA la retoma con lo conversado. */
  async function openPast(id: string) {
    if (busy) return;
    try {
      const [past, recap] = await Promise.all([
        invoke<SavedConversation>("conversations_get", { id }),
        invoke<string>("conversations_recap", { id }),
      ]);
      session.current = null;
      request.current = "";
      conversation.current = past.id;
      setPending([]);
      setRuns([]);
      setQueue([]);
      seen.current = 0;
      setSlug(past.slug ?? "");
      setEntries(
        past.items.map((m): Entry =>
          m.role === "usuario"
            ? { role: "user", text: m.text }
            : m.role === "ia"
              ? { role: "assistant", text: m.text, who: m.author || undefined }
              : { role: "error", text: m.text },
        ),
      );
      carry.current = `Retomamos una conversación anterior. Esto fue lo conversado:\n\n${recap}\n\nSigamos desde ahí.`;
      setShowHistory(false);
      input.current?.focus();
    } catch (e) {
      setEntries((all) => [...all, { role: "error", text: String(e) }]);
    }
  }

  // Los controles de Mejoras y Decisiones: lo mismo que en esas vistas, sin salir del chat.
  const projectName = (s: string) => contexts.find((c) => c.project.slug === s)?.project.name ?? s;
  const canWork = (s: string) => Boolean(contexts.find((c) => c.project.slug === s)?.project.local_path);
  const board: BoardHandlers = {
    draft: (text) => {
      setDraft(text);
      input.current?.focus();
    },
    workHere: (item) => {
      const message = [
        `Trabaja aquí la mejora #${item.id} del proyecto «${projectName(item.slug)}» (slug \`${item.slug}\`): «${item.title}».`,
        `Detalle: ${item.detail || "(sin detalle)"}`,
        item.result ? `Lo que dejó el último intento:\n${item.result}` : "",
        item.feedback ? `Lo que ya respondí antes:\n${item.feedback}` : "",
        "Hazla completa y llévala por sus etapas en el tablero (en curso, en revisión con la rama o el PR, hecha).",
      ]
        .filter(Boolean)
        .join("\n\n");
      const shown = `Haz aquí la mejora #${item.id} «${item.title}»`;
      if (busy) setQueue((all) => [...all, { text: message, files: [] }]);
      else dispatch(message, shown);
    },
    answer: (item) => {
      setAsking(item);
      input.current?.focus();
    },
    changed: () => {
      refresh();
      reloadDecisions();
      reloadMemories();
    },
  };
  boardChanged.current = board.changed;
  // Si un paso toca varias veces la misma mejora, sus botones van solo bajo el último.
  const lastTouch = new Map<string, number>();
  entries.forEach((e, i) => {
    if (e.role === "tool" && e.target) lastTouch.set(`${e.target.kind}-${e.target.id}`, i);
  });
  const multi = team.length > 1;
  const workingNow = team.filter((m) => m.working && m.kind !== "principal");

  const reload = () =>
    invoke<Approval[]>("chat_pending", { conversation: conversation.current })
      .then(setPending)
      .catch(() => {});

  return (
    <div className="chat" style={visible ? undefined : { display: "none" }}>
      <header className="page-head">
        <div>
          <h1>Agente</h1>
          <p>
            {isClaude || isAgent
              ? current.note
              : "Este modelo conversa con el estado de tus proyectos ya armado, pero no cambia nada. Para que trabaje en tus proyectos, elige Claude Code u otro agente."}
          </p>
        </div>
        <div className="actions">
          <select
            value={model}
            onChange={(e) => {
              setModel(e.target.value);
              // Otro modelo empieza otra conversación.
              session.current = null;
            }}
            disabled={busy}
            title="Quién responde"
          >
            <option value="">Asistente ({usual})</option>
            {models.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
          {(isClaude || isAgent) && (
            <select
              value={effective}
              onChange={(e) => chooseMode(e.target.value as Mode)}
              disabled={busy}
              title="Qué puede hacer"
            >
              {modes.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.label}
                </option>
              ))}
            </select>
          )}
          <select value={slug} onChange={(e) => setSlug(e.target.value)} disabled={busy} title="Proyecto en foco">
            <option value="">Todos los proyectos</option>
            {contexts.map((c) => (
              <option key={c.project.slug} value={c.project.slug}>
                {c.project.name}
              </option>
            ))}
          </select>
          <button
            className={`btn${comparing ? " primary" : ""}`}
            onClick={() => setComparing((c) => (c ? null : []))}
            disabled={busy}
            title="Haz la misma pregunta a varias IA a la vez y compara sus respuestas"
          >
            Comparar
          </button>
          <button
            className={`btn${showHistory ? " primary" : ""}`}
            onClick={() => setShowHistory((v) => !v)}
            disabled={busy}
            title="Busca, retoma o exporta conversaciones anteriores"
          >
            Historial
          </button>
          <button className="btn" onClick={reset} disabled={!entries.length || busy}>
            Nueva conversación
          </button>
        </div>
      </header>

      {comparing && (
        <div className="chat-comparar">
          <span className="stamp">Pregunta a la vez a (de 2 a {COMPARE_MAX}; solo leen, no cambian nada):</span>
          {models.map((m) => {
            const on = comparing.includes(m.id);
            return (
              <label key={m.id} className={on ? "on" : undefined}>
                <input
                  type="checkbox"
                  checked={on}
                  disabled={busy || (!on && comparing.length >= COMPARE_MAX)}
                  onChange={() => setComparing((c) => (c ? (on ? c.filter((x) => x !== m.id) : [...c, m.id]) : c))}
                />
                {m.name}
              </label>
            );
          })}
        </div>
      )}

      {showHistory && (
        <ChatHistory
          projectName={projectName}
          current={conversation.current}
          onOpen={openPast}
          onClose={() => setShowHistory(false)}
        />
      )}

      <Team members={team} />
      <BoardBar
        improvements={improvements}
        decisions={decisions}
        memories={memories}
        slug={slug}
        projectName={projectName}
        canWork={canWork}
        on={board}
      />

      <section className="panel chat-log">
        {asking && (
          <div className="chat-asking">
            <div className="chat-who">
              Mejora #{asking.id} · {contexts.find((c) => c.project.slug === asking.slug)?.project.name ?? asking.slug} · necesita
              tu respuesta
              <button type="button" className="link" onClick={() => setAsking(null)} title="Escribir otra cosa">
                Ahora no
              </button>
            </div>
            <strong>{asking.title}</strong>
            <p className="card-question">{askedOf(asking)}</p>
            <p className="stamp">
              Respóndele abajo: {usual} retoma la mejora con tu respuesta y la lleva hasta el final, moviéndola por el tablero.
            </p>
          </div>
        )}
        {entries.length === 0 && !asking && (
          <div className="chat-empty">
            <Tag>{model ? models.find((m) => m.id === model)?.name : usual}</Tag>
            <p>Prueba con:</p>
            <div className="suggestions">
              {SUGGESTIONS.map((s) => (
                <button key={s} className="btn" onClick={() => send(s)}>
                  {s}
                </button>
              ))}
            </div>
          </div>
        )}
        {entries.map((entry, i) => {
          if (entry.role === "tool") {
            const nested = !!entry.parent;
            const sub = entry.actor && entry.actor !== CLAUDE;
            const live = entry.id ? runs.filter((r) => r.tool === entry.id) : [];
            return (
              <div key={i} className={nested ? "chat-nested" : undefined}>
                <div className={`chat-tool${busy && entry.id && !entry.done ? " running" : ""}${entry.failed ? " failed" : ""}`}>
                  {multi && entry.actor && <span className={`chat-actor ${sub ? "subagente" : "principal"}`}>{entry.actor}</span>}
                  ▸ {entry.spawns ? `Lanza a ${entry.spawns}` : (TOOL_LABEL[entry.text] ?? entry.text)}
                  {entry.detail && !entry.spawns && <code className="chat-tool-detail">{entry.detail}</code>}
                </div>
                {/* Un reparto: los encargos corren a la vez, así que se ven lado a lado. */}
                <div className={live.length > 1 ? "chat-live-group" : undefined}>
                  {live.map((r) => (
                    <LiveRun key={r.run} run={r} />
                  ))}
                </div>
                {entry.target && lastTouch.get(`${entry.target.kind}-${entry.target.id}`) === i && (
                  <ItemCard
                    target={entry.target}
                    improvements={improvements}
                    decisions={decisions}
                    memories={memories}
                    projectName={projectName}
                    canWork={canWork}
                    on={board}
                  />
                )}
              </div>
            );
          }
          if (entry.role === "compare")
            return (
              <div key={i} className="chat-compare">
                {entry.names.map((name, j) => {
                  const a = entry.answers[j];
                  return (
                    <article key={j} className={`chat-compare-card${a && !a.ok ? " error" : ""}`}>
                      <header>
                        <strong>{a?.name ?? name}</strong>
                        <span className="stamp">
                          {!a
                            ? "pensando…"
                            : !a.ok
                              ? "no respondió"
                              : `${a.seconds} s · ${a.cost_usd != null ? `US$ ${a.cost_usd.toFixed(4)}` : "costo no informado"}`}
                        </span>
                      </header>
                      {!a ? (
                        <div className="chat-tool running">▸ Pensando…</div>
                      ) : a.ok ? (
                        <div className="chat-text">
                          <Rich text={a.text} />
                        </div>
                      ) : (
                        <p className="tablero-error">{a.text}</p>
                      )}
                      {a?.ok && !busy && (
                        <button type="button" className="link" onClick={() => continueWith(entry.question, a)}>
                          Seguir con {a.name}
                        </button>
                      )}
                    </article>
                  );
                })}
              </div>
            );
          if (entry.role === "output")
            return (
              <div key={i} className={entry.parent ? "chat-nested" : undefined}>
                <Output text={entry.text} failed={entry.failed} />
              </div>
            );
          return (
            <div key={i} className={`chat-msg ${entry.role}`}>
              <div className="chat-who">
                {entry.role === "user"
                  ? "Tú"
                  : entry.role !== "assistant"
                    ? "Error"
                    : entry.who
                      ? `Atalaya · ${entry.who}`
                      : "Atalaya"}
              </div>
              <div className="chat-text">{entry.role === "assistant" ? <Rich text={entry.text} /> : entry.text}</div>
              {entry.role === "user" && entry.files && <FileStrip files={entry.files} />}
            </div>
          );
        })}
        {pending.map((p) => (
          <ApprovalCard
            key={p.id}
            item={p}
            who={multi ? entries.find((e): e is Tool => e.role === "tool" && !!p.step && e.id === p.step)?.actor : undefined}
            onAnswered={reload}
          />
        ))}
        {runs
          .filter((r) => !r.tool || !entries.some((e) => e.role === "tool" && e.id === r.tool))
          .map((r) => (
            <LiveRun key={r.run} run={r} />
          ))}
        {busy && !pending.length && entries[entries.length - 1]?.role !== "compare" && (
          <div className="chat-tool pending">
            ▸{" "}
            {workingNow.length
              ? `${main} espera a ${workingNow.map((m) => m.name).join(", ")}…`
              : multi
                ? `${main} trabajando…`
                : "Trabajando…"}
          </div>
        )}
        {queue.map((q, i) => (
          <div key={i} className="chat-msg user queued">
            <div className="chat-who">
              Tú · {busy ? "sale cuando termine lo que está haciendo" : "sin enviar"}
              <button
                type="button"
                className="link"
                onClick={() => setQueue((all) => all.filter((_, j) => j !== i))}
                title="No lo envía"
              >
                Quitar
              </button>
            </div>
            <div className="chat-text">{q.text}</div>
            <FileStrip files={q.files} />
          </div>
        ))}
        <div ref={end} />
      </section>

      {(attach.files.length > 0 || attach.error || attach.adding) && (
        <div className="chat-adjuntando">
          <FileStrip files={attach.files} onRemove={attach.remove} />
          {attach.adding && <span className="stamp">Adjuntando…</span>}
          {attach.error && (
            <p className="tablero-error">
              {attach.error}{" "}
              <button type="button" className="link" onClick={attach.dismissError}>
                Cerrar
              </button>
            </p>
          )}
        </div>
      )}
      {suggestions.length > 0 && (
        <ul className="chat-comandos" role="listbox" aria-label="Comandos guardados">
          {suggestions.map((c) => (
            <li key={c.name}>
              <button type="button" onClick={() => (setDraft(`/${c.name} `), input.current?.focus())}>
                <code>/{c.name}</code> <span className="stamp">{c.description}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <form
        className={`chat-input${attach.dragging ? " soltando" : ""}`}
        onSubmit={(e) => {
          e.preventDefault();
          send(draft);
        }}
      >
        <button
          type="button"
          className="btn chat-clip"
          hidden={!!comparing}
          onClick={attach.pick}
          title="Adjuntar archivos. También puedes pegar una captura (⌘V) o arrastrarlos aquí."
          aria-label="Adjuntar archivos"
        >
          📎
        </button>
        <textarea
          ref={input}
          value={draft}
          onPaste={attach.onPaste}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if ((e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) && suggestions.length && typed !== suggestions[0].name) {
              e.preventDefault();
              setDraft(`/${suggestions[0].name} `);
              return;
            }
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send(draft);
            }
          }}
          placeholder={
            attach.dragging
              ? "Suelta los archivos para adjuntarlos."
              : comparing
                ? comparing.length < 2
                  ? "Elige arriba al menos dos IA para comparar."
                  : `La misma pregunta va a ${comparing.length} IA a la vez. Enter envía.`
                : asking
                  ? `Tu respuesta para la mejora #${asking.id}. Enter envía.`
                  : busy
                    ? "Escribe lo que sigue: lo recibe al terminar este paso. Para cambiar de rumbo ya, envíalo y pulsa Detener."
                    : (isClaude || isAgent) && effective !== "lectura"
                      ? "Pídele lo que harías en la terminal: revisar, arreglar, actualizar, publicar. Enter envía, Mayús+Enter salta de línea."
                      : "Escribe tu pregunta. Enter envía, Mayús+Enter salta de línea."
          }
          rows={2}
          autoFocus
        />
        <button
          className="btn primary"
          disabled={(!draft.trim() && !attach.files.length) || (!!comparing && (busy || comparing.length < 2))}
          title={busy ? "Sale cuando termine lo que está haciendo" : undefined}
        >
          Enviar
        </button>
        {busy && entries[entries.length - 1]?.role !== "compare" && (
          <button
            type="button"
            className="btn"
            onClick={stop}
            title={queue.length ? "Lo detiene y envía lo que escribiste" : undefined}
          >
            Detener
          </button>
        )}
      </form>
    </div>
  );
}
