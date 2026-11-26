import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Console } from "../App";
import { Empty, Panel, Rich, Tag } from "../ui";

/**
 * Lo que Atalaya hace solo (ver `automations.rs`): tareas programadas y reacciones a las
 * fallas. Las dos solo leen; lo que encuentran llega como aviso y queda en el historial.
 */

type Schedule = {
  id: number;
  name: string;
  slug: string | null;
  prompt: string;
  ai: string | null;
  every: "dia" | "semana" | "horas";
  at: string;
  weekday: number | null;
  hours: number | null;
  enabled: boolean;
  last_run: string | null;
};

type Reaction = {
  id: number;
  name: string;
  kinds: string[];
  slug: string | null;
  ai: string | null;
  improvement: boolean;
  enabled: boolean;
};

type Run = {
  id: number;
  source: "tarea" | "reaccion";
  source_id: number;
  name: string;
  slug: string | null;
  kind: string | null;
  ai: string;
  ok: boolean;
  text: string;
  improvement_id: number | null;
  at: string;
};

type Data = { schedules: Schedule[]; reactions: Reaction[]; runs: Run[]; kinds: string[] };
type Option = { id: string; name: string };
type Roles = { roles: { options: Option[] }[] };

const WEEKDAYS = ["domingo", "lunes", "martes", "miércoles", "jueves", "viernes", "sábado"];

function when(s: Schedule) {
  if (s.every === "semana") return `Cada ${WEEKDAYS[s.weekday ?? 1]} a las ${s.at}`;
  if (s.every === "horas") return `Cada ${s.hours} ${s.hours === 1 ? "hora" : "horas"}`;
  return `Cada día a las ${s.at}`;
}

const EMPTY_SCHEDULE: Schedule = {
  id: 0,
  name: "",
  slug: null,
  prompt: "",
  ai: null,
  every: "dia",
  at: "09:00",
  weekday: 1,
  hours: 6,
  enabled: true,
  last_run: null,
};

const EMPTY_REACTION: Reaction = {
  id: 0,
  name: "",
  kinds: ["despliegue"],
  slug: null,
  ai: null,
  improvement: false,
  enabled: true,
};

/** Botones para pausar, quitar (con confirmación) y, en las tareas, correr ahora. */
function RowActions({
  enabled,
  onToggle,
  onEdit,
  onRemove,
  onRun,
  running,
}: {
  enabled: boolean;
  onToggle: () => void;
  onEdit: () => void;
  onRemove: () => void;
  onRun?: () => void;
  running?: boolean;
}) {
  const [removing, setRemoving] = useState(false);
  return (
    <div className="skill-actions">
      {onRun && (
        <button type="button" className="btn" disabled={running} onClick={onRun}>
          {running ? "Corriendo…" : "Correr ahora"}
        </button>
      )}
      <button type="button" className="btn" onClick={onEdit}>
        Editar
      </button>
      <button type="button" className="btn" onClick={onToggle}>
        {enabled ? "Pausar" : "Reanudar"}
      </button>
      {removing ? (
        <>
          <button type="button" className="btn danger" onClick={onRemove}>
            Sí, quitar
          </button>
          <button type="button" className="btn" onClick={() => setRemoving(false)}>
            No
          </button>
        </>
      ) : (
        <button type="button" className="btn" onClick={() => setRemoving(true)} title="El historial queda">
          Quitar
        </button>
      )}
    </div>
  );
}

function ScheduleForm({
  draft,
  setDraft,
  projects,
  ais,
  onSave,
  onCancel,
}: {
  draft: Schedule;
  setDraft: (s: Schedule) => void;
  projects: Option[];
  ais: Option[];
  onSave: () => void;
  onCancel: () => void;
}) {
  return (
    <form
      className="form"
      onSubmit={(e) => {
        e.preventDefault();
        onSave();
      }}
    >
      <div className="form-row">
        <label>
          Nombre
          <input
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            placeholder="Resumen del lunes"
          />
        </label>
        <label>
          Proyecto
          <select value={draft.slug ?? ""} onChange={(e) => setDraft({ ...draft, slug: e.target.value || null })}>
            <option value="">Todos</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          IA
          <select value={draft.ai ?? ""} onChange={(e) => setDraft({ ...draft, ai: e.target.value || null })}>
            <option value="">La del validador</option>
            {ais.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
        </label>
      </div>
      <label>
        Qué tiene que hacer
        <textarea
          rows={3}
          value={draft.prompt}
          onChange={(e) => setDraft({ ...draft, prompt: e.target.value })}
          placeholder="Revisa si hay dependencias con fallas conocidas y dime cuáles subir. También sirve un comando guardado: /resumen-semana faro"
        />
      </label>
      <div className="form-row">
        <label>
          Cada
          <select value={draft.every} onChange={(e) => setDraft({ ...draft, every: e.target.value as Schedule["every"] })}>
            <option value="dia">Día</option>
            <option value="semana">Semana</option>
            <option value="horas">Tantas horas</option>
          </select>
        </label>
        {draft.every === "semana" && (
          <label>
            Día
            <select value={draft.weekday ?? 1} onChange={(e) => setDraft({ ...draft, weekday: Number(e.target.value) })}>
              {WEEKDAYS.map((d, i) => (
                <option key={d} value={i}>
                  {d}
                </option>
              ))}
            </select>
          </label>
        )}
        {draft.every === "horas" ? (
          <label>
            Horas
            <input
              type="number"
              min={1}
              max={168}
              value={draft.hours ?? 6}
              onChange={(e) => setDraft({ ...draft, hours: Number(e.target.value) })}
            />
          </label>
        ) : (
          <label>
            Hora
            <input type="time" value={draft.at} onChange={(e) => setDraft({ ...draft, at: e.target.value })} />
          </label>
        )}
      </div>
      <div className="form-actions">
        {draft.id !== 0 && (
          <button type="button" className="btn" onClick={onCancel}>
            Cancelar
          </button>
        )}
        <button className="btn primary" disabled={!draft.name.trim() || !draft.prompt.trim()}>
          {draft.id ? "Guardar cambios" : "Programar tarea"}
        </button>
      </div>
    </form>
  );
}

function ReactionForm({
  draft,
  setDraft,
  kinds,
  projects,
  ais,
  onSave,
  onCancel,
}: {
  draft: Reaction;
  setDraft: (r: Reaction) => void;
  kinds: string[];
  projects: Option[];
  ais: Option[];
  onSave: () => void;
  onCancel: () => void;
}) {
  return (
    <form
      className="form"
      onSubmit={(e) => {
        e.preventDefault();
        onSave();
      }}
    >
      <div className="form-row">
        <label>
          Nombre
          <input
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            placeholder="Despliegues caídos"
          />
        </label>
        <label>
          Proyecto
          <select value={draft.slug ?? ""} onChange={(e) => setDraft({ ...draft, slug: e.target.value || null })}>
            <option value="">Todos</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          IA
          <select value={draft.ai ?? ""} onChange={(e) => setDraft({ ...draft, ai: e.target.value || null })}>
            <option value="">La del validador</option>
            {ais.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name}
              </option>
            ))}
          </select>
        </label>
      </div>
      <div>
        <span className="stamp">Cuando falle (sin marcar ninguna, cualquiera):</span>
        <div className="chat-comparar">
          {kinds.map((k) => {
            const on = draft.kinds.includes(k);
            return (
              <label key={k} className={on ? "on" : undefined}>
                <input
                  type="checkbox"
                  checked={on}
                  onChange={() => setDraft({ ...draft, kinds: on ? draft.kinds.filter((x) => x !== k) : [...draft.kinds, k] })}
                />
                {k}
              </label>
            );
          })}
        </div>
      </div>
      <label className="check">
        <input
          type="checkbox"
          checked={draft.improvement}
          onChange={(e) => setDraft({ ...draft, improvement: e.target.checked })}
        />
        Dejar el diagnóstico como mejora propuesta
      </label>
      <div className="form-actions">
        {draft.id !== 0 && (
          <button type="button" className="btn" onClick={onCancel}>
            Cancelar
          </button>
        )}
        <button className="btn primary" disabled={!draft.name.trim()}>
          {draft.id ? "Guardar cambios" : "Agregar reacción"}
        </button>
      </div>
    </form>
  );
}

function RunRow({ run, projectName }: { run: Run; projectName: (s: string) => string }) {
  const [open, setOpen] = useState(false);
  const first = (run.text.split("\n").find((l) => l.trim()) ?? "").replace(/\*\*/g, "");
  return (
    <li className="skill">
      <div className="skill-head">
        <Tag tone={run.ok ? undefined : "brasa"}>{run.source === "tarea" ? "Tarea" : "Reacción"}</Tag>
        <strong>{run.name}</strong>
        <span className="stamp">
          {run.slug ? `${projectName(run.slug)} · ` : ""}
          {run.kind ? `${run.kind} · ` : ""}
          {run.ai || "…"} · {run.at.slice(0, 16)}
          {run.improvement_id ? ` · mejora #${run.improvement_id}` : ""}
        </span>
        <div className="skill-actions">
          <button type="button" className="link" onClick={() => setOpen(!open)}>
            {open ? "Cerrar" : "Ver"}
          </button>
        </div>
      </div>
      {open ? (
        <div className="chat-text">
          <Rich text={run.text} />
        </div>
      ) : (
        <p className="skill-description clamp">{first}</p>
      )}
    </li>
  );
}

export function Tareas({ contexts, system }: Console) {
  const [data, setData] = useState<Data | null>(null);
  const [ais, setAis] = useState<Option[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [schedule, setSchedule] = useState<Schedule>(EMPTY_SCHEDULE);
  const [reaction, setReaction] = useState<Reaction>(EMPTY_REACTION);
  const [running, setRunning] = useState<number | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await invoke<Data>("automations_list"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
    invoke<Roles>("ai_roles")
      .then((r) => {
        const seen = new Map<string, string>();
        r.roles.forEach((role) => role.options.forEach((o) => seen.set(o.id, o.name)));
        setAis([...seen].map(([id, name]) => ({ id, name })));
      })
      .catch(() => {});
  }, [load]);

  const act = async (task: () => Promise<unknown>) => {
    try {
      await task();
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    load();
  };
  const projects: Option[] = contexts.map((c) => ({ id: c.project.slug, name: c.project.name }));
  const projectName = (s: string) => projects.find((p) => p.id === s)?.name ?? s;
  const aiName = (id: string | null) => (id ? (ais.find((a) => a.id === id)?.name ?? id) : "la del validador");

  return (
    <>
      <header className="page-head">
        <div>
          <h1>Tareas</h1>
          <p>
            Lo que Atalaya hace solo: encargos que corren cada día, semana o tantas horas, y diagnósticos apenas falla algo. Solo
            leen: lo que encuentran te llega como aviso (y por Telegram, si lo tienes) y queda abajo.
          </p>
        </div>
      </header>
      {system?.service_running === false && (
        <div className="banner">La Vigía está detenida: las tareas y reacciones corren con ella. Enciéndela en Ajustes.</div>
      )}
      {error && <div className="banner">{error}</div>}

      <Panel title={`Tareas programadas · ${data?.schedules.length ?? 0}`}>
        {data && data.schedules.length === 0 && <Empty>Todavía no hay tareas.</Empty>}
        {data && data.schedules.length > 0 && (
          <ul className="skill-list">
            {data.schedules.map((s) => (
              <li key={s.id} className="skill">
                <div className="skill-head">
                  <strong>{s.name}</strong>
                  {!s.enabled && <Tag>Pausada</Tag>}
                  <span className="stamp">
                    {when(s)} · {s.slug ? projectName(s.slug) : "todos los proyectos"} · {aiName(s.ai)}
                  </span>
                  <RowActions
                    enabled={s.enabled}
                    running={running === s.id}
                    onRun={() => {
                      setRunning(s.id);
                      act(() => invoke("schedules_run", { id: s.id })).finally(() => setRunning(null));
                    }}
                    onEdit={() => setSchedule(s)}
                    onToggle={() => act(() => invoke("schedules_enable", { id: s.id, on: !s.enabled }))}
                    onRemove={() => act(() => invoke("schedules_remove", { id: s.id }))}
                  />
                </div>
                <p className="skill-description clamp">{s.prompt}</p>
              </li>
            ))}
          </ul>
        )}
        <ScheduleForm
          draft={schedule}
          setDraft={setSchedule}
          projects={projects}
          ais={ais}
          onCancel={() => setSchedule(EMPTY_SCHEDULE)}
          onSave={() =>
            act(async () => {
              await invoke("schedules_save", { input: schedule });
              setSchedule(EMPTY_SCHEDULE);
            })
          }
        />
      </Panel>

      <Panel title={`Reacciones · ${data?.reactions.length ?? 0}`}>
        <p className="stamp">
          Cuando una revisión pasa a falla, la IA que elijas diagnostica la causa apenas la Vigía lo detecta (los despliegues y el
          sitio se revisan cada 5 minutos). Para el mismo proyecto y revisión no se repite antes de 6 horas.
        </p>
        {data && data.reactions.length === 0 && <Empty>Todavía no hay reacciones.</Empty>}
        {data && data.reactions.length > 0 && (
          <ul className="skill-list">
            {data.reactions.map((x) => (
              <li key={x.id} className="skill">
                <div className="skill-head">
                  <strong>{x.name}</strong>
                  {!x.enabled && <Tag>Pausada</Tag>}
                  {x.improvement && <Tag tone="almenara">Deja mejora</Tag>}
                  <span className="stamp">
                    {x.kinds.length ? x.kinds.join(", ") : "cualquier revisión"} · {x.slug ? projectName(x.slug) : "todos"} ·{" "}
                    {aiName(x.ai)}
                  </span>
                  <RowActions
                    enabled={x.enabled}
                    onEdit={() => setReaction(x)}
                    onToggle={() => act(() => invoke("reactions_enable", { id: x.id, on: !x.enabled }))}
                    onRemove={() => act(() => invoke("reactions_remove", { id: x.id }))}
                  />
                </div>
              </li>
            ))}
          </ul>
        )}
        {data && (
          <ReactionForm
            draft={reaction}
            setDraft={setReaction}
            kinds={data.kinds}
            projects={projects}
            ais={ais}
            onCancel={() => setReaction(EMPTY_REACTION)}
            onSave={() =>
              act(async () => {
                await invoke("reactions_save", { input: reaction });
                setReaction(EMPTY_REACTION);
              })
            }
          />
        )}
      </Panel>

      <Panel title="Historial">
        {data && data.runs.length === 0 && <Empty>Nada todavía.</Empty>}
        {data && data.runs.length > 0 && (
          <ul className="skill-list">
            {data.runs.map((r) => (
              <RunRow key={r.id} run={r} projectName={projectName} />
            ))}
          </ul>
        )}
      </Panel>
    </>
  );
}
