import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Console } from "../App";
import { Empty, Panel, Pip } from "../ui";
import { Conectar } from "./Conectar";

type Chosen = { id: string; name: string; ready: boolean; reason: string | null };
type Role = {
  id: string;
  label: string;
  description: string;
  strategy: string;
  candidates: Chosen[];
  options: { id: string; name: string; free?: boolean }[];
};
type Roles = { roles: Role[]; strategies: { id: string; label: string }[] };

const ROLE_NOTE: Record<string, string> = {
  constructor:
    "Claude Code o un agente de terminal (Codex, Antigravity, Qwen Code con un modelo de tu Mac…). Los que no son Claude Code trabajan dentro del aislamiento de macOS.",
  revisor:
    "Un modelo o agente distinto al que construyó lee el cambio y da su veredicto. Si pide cambios, el constructor tiene un intento más. Vacío: nadie revisa.",
  validador:
    "Claude Code usa las herramientas de Atalaya; los demás reciben el estado de los proyectos en JSON. Los agentes de terminal responden en solo lectura.",
  asistente: "Responde en Agente cuando no eliges otro en la conversación.",
};

/** Un rol: su lista ordenada, la estrategia y qué se puede agregar. */
function RoleEditor({ role, strategies, onSaved }: { role: Role; strategies: Roles["strategies"]; onSaved: (r: Roles) => void }) {
  const [list, setList] = useState(role.candidates.map((c) => c.id));
  const [strategy, setStrategy] = useState(role.strategy);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setList(role.candidates.map((c) => c.id));
    setStrategy(role.strategy);
  }, [role]);

  const dirty = strategy !== role.strategy || list.join() !== role.candidates.map((c) => c.id).join();
  const name = (id: string) =>
    role.candidates.find((c) => c.id === id)?.name ?? role.options.find((o) => o.id === id)?.name ?? id;
  const status = (id: string) => role.candidates.find((c) => c.id === id);
  const free = role.options.filter((o) => !list.includes(o.id));

  const move = (i: number, by: number) => {
    const next = [...list];
    const [item] = next.splice(i, 1);
    next.splice(i + by, 0, item);
    setList(next);
  };

  const save = async () => {
    setBusy(true);
    try {
      onSaved(await invoke<Roles>("ai_set_role", { role: role.id, candidates: list, strategy }));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Panel
      title={role.label}
      aside={
        <select value={strategy} onChange={(e) => setStrategy(e.target.value)} aria-label={`Estrategia de ${role.label}`}>
          {strategies.map((s) => (
            <option key={s.id} value={s.id}>
              {s.label}
            </option>
          ))}
        </select>
      }
      flush
    >
      <div className="panel-body">
        <p>{role.description}</p>
        <p className="footnote">{ROLE_NOTE[role.id]}</p>
      </div>
      {list.length ? (
        <table className="table">
          <tbody>
            {list.map((id, i) => {
              const s = status(id);
              return (
                <tr key={id}>
                  <td className="shrink stamp">{i + 1}</td>
                  <td>
                    <span className="cell-name">
                      <Pip status={s ? (s.ready ? "ok" : "aviso") : undefined} />
                      {name(id)}
                    </span>
                    {s && !s.ready && <div className="stamp almenara">No disponible: {s.reason}</div>}
                    {!s && <div className="stamp">Se comprueba al guardar</div>}
                  </td>
                  <td className="shrink">
                    <div className="row-actions">
                      <button className="btn" disabled={i === 0} onClick={() => move(i, -1)} aria-label="Subir">
                        ↑
                      </button>
                      <button className="btn" disabled={i === list.length - 1} onClick={() => move(i, 1)} aria-label="Bajar">
                        ↓
                      </button>
                      <button className="btn" onClick={() => setList(list.filter((x) => x !== id))}>
                        Quitar
                      </button>
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      ) : (
        <div className="panel-body">
          <Empty>
            {role.id === "revisor" ? "Nadie revisa los cambios del constructor." : "Sin asignar: este trabajo no se hará."}
          </Empty>
        </div>
      )}
      <div className="form-actions spread pad">
        {free.length ? (
          <select
            value=""
            onChange={(e) => e.target.value && setList([...list, e.target.value])}
            aria-label={`Agregar a ${role.label}`}
          >
            <option value="">Agregar…</option>
            {free.map((o) => (
              <option key={o.id} value={o.id}>
                {o.name}
                {o.free ? " · gratis" : ""}
              </option>
            ))}
          </select>
        ) : (
          <span className="stamp">No hay más candidatos para este rol.</span>
        )}
        <button className="btn primary" disabled={!dirty || busy} onClick={save}>
          {busy ? "Guardando…" : "Guardar"}
        </button>
      </div>
      {error && <div className="error-note">{error}</div>}
    </Panel>
  );
}

type Member = {
  id: string;
  name: string;
  roles: string[];
  free: boolean;
  can_edit: boolean;
  unavailable: string | null;
};
type Coordination = { enabled: boolean; team: Member[] };

/**
 * «Claude Code coordina»: en el chat, Claude Code le encarga consultas y cambios a las demás IA de tus roles.
 * Viene apagado: lo que eliges en los roles manda, y esto solo agrega a quién puede pedirle ayuda.
 */
function Coordina({ roles }: { roles: Roles }) {
  const [data, setData] = useState<Coordination | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    invoke<Coordination>("ai_coordination")
      .then(setData)
      .catch((e) => setError(String(e)));
  }, [roles]);

  if (!data) return error ? <div className="error-note">{error}</div> : null;
  const toggle = async (on: boolean) => {
    setBusy(true);
    try {
      setData(await invoke<Coordination>("ai_set_coordination", { on }));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const label = (id: string) => roles.roles.find((r) => r.id === id)?.label.toLowerCase() ?? id;

  return (
    <Panel
      title="Claude Code coordina"
      aside={
        <label className="check">
          <input type="checkbox" checked={data.enabled} disabled={busy} onChange={(e) => toggle(e.target.checked)} />
          {data.enabled ? "Encendido" : "Apagado"}
        </label>
      }
      flush
    >
      <div className="panel-body">
        <p>
          En el chat, Claude Code reparte el trabajo: le pide análisis y revisiones a las demás IA de tus roles y les encarga
          cambios acotados en la carpeta de un proyecto. Él revisa lo que devuelven y te responde. Así gasta menos de tu plan de
          Claude y prefiere a las gratis.
        </p>
        <p className="footnote" style={{ wordBreak: "normal" }}>
          Solo le pide a quienes están en tus roles. Encargar un cambio espera tu visto bueno en «Puede todo, pregunta antes de
          cambiar», y en solo lectura solo puede consultar. Cada encargo aparece en el chat y en Consumo como «Delegado».
        </p>
      </div>
      {data.team.length ? (
        <table className="table">
          <tbody>
            {data.team.map((m) => (
              <tr key={m.id}>
                <td>
                  <span className="cell-name">
                    <Pip status={m.unavailable ? "aviso" : "ok"} />
                    {m.name}
                  </span>
                  {m.unavailable && <div className="stamp almenara">No disponible: {m.unavailable}</div>}
                </td>
                <td className="stamp">
                  {m.free ? "gratis" : "de pago o con su cuenta"} · {m.can_edit ? "consulta y cambia archivos" : "solo consulta"}
                </td>
                <td className="stamp">{m.roles.map(label).join(", ")}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <div className="panel-body">
          <Empty>Tus roles solo tienen a Claude Code. Agrega otras IA a los roles para que tenga a quién pedirle ayuda.</Empty>
        </div>
      )}
      {error && <div className="error-note">{error}</div>}
    </Panel>
  );
}

/** Quién hace cada trabajo: constructor, revisor, validador y asistente. */
export function IA({ go }: Console) {
  const [data, setData] = useState<Roles | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await invoke<Roles>("ai_roles"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  if (error) return <div className="banner">{error}</div>;
  if (!data) return <Empty>Cargando roles…</Empty>;

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>IA</h1>
          <p>
            Conecta tus IA y elige quién hace cada trabajo. Cada rol tiene una lista en orden: si el elegido no está, falla o pasó
            su presupuesto, Atalaya sigue con el siguiente. El revisor nunca es el mismo modelo que construyó.
          </p>
        </div>
        <button className="btn" onClick={() => go("consumo")}>
          Ver consumo
        </button>
      </header>

      <Conectar onChange={load} goApi={() => go("conexiones")} />

      <header className="page-head">
        <div>
          <h2>Quién hace cada trabajo</h2>
          <p>Todo lo que conectaste aparece en «Agregar…» de cada rol.</p>
        </div>
      </header>
      <div className="split">
        {data.roles.map((role) => (
          <RoleEditor key={role.id} role={role} strategies={data.strategies} onSaved={setData} />
        ))}
      </div>
      <Coordina roles={data} />
    </div>
  );
}
