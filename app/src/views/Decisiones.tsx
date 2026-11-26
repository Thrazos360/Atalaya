import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Console } from "../App";
import { ago } from "../api";
import { Empty, Rich, Tag } from "../ui";

type Decision = {
  id: number;
  slug: string;
  kind: string;
  title: string;
  context: string;
  proposal: string;
  action: "mejora" | "comentario" | "reconocer" | "actualizar" | "ninguna";
  target: string | null;
  status: "pendiente" | "aprobada" | "descartada";
  result: string | null;
  created_at: string;
};

type Payload = {
  decisions: Decision[];
  round: { at: string | null; summary: string | null; auto: boolean };
  telegram: { configured: boolean; decisions: boolean };
};

/** Qué pasa al aprobar, dicho en simple. `canWork`: el proyecto tiene carpeta donde el agente puede trabajar. */
function effect(d: Decision, canWork: boolean) {
  switch (d.action) {
    case "mejora":
      return "Al aprobar se crea una mejora y Claude empieza a trabajarla.";
    case "comentario":
      return `Al aprobar se publica este texto como comentario en el issue #${d.target} de GitHub.`;
    case "reconocer":
      return `Al aprobar se reconoce la alerta de ${d.target}: sigue vigilada, pero deja de contar como abierta.`;
    case "actualizar":
      return "Al aprobar, Atalaya trae lo último de la rama principal a tu carpeta local. Solo avanza si no pisa nada tuyo; si no puede, te dice qué lo impide. El Vigía también lo reintenta solo en cada vuelta.";
    default:
      return canWork
        ? "Al aprobar, Claude la toma como mejora y la trabaja en una rama aparte. Lo que no se pueda resolver desde el código te lo deja explicado en su informe."
        : "Este proyecto no tiene carpeta en este Mac, así que el agente no puede trabajarla: solo se puede dar por vista.";
  }
}

const ACTION_LABEL = {
  mejora: "Crear mejora",
  comentario: "Comentar issue",
  reconocer: "Reconocer alerta",
  actualizar: "Poner al día",
  ninguna: "Por resolver",
};

export function Decisiones({ contexts, refresh, openProject, go }: Console) {
  const [data, setData] = useState<Payload | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<"ronda" | "decision" | null>(null);
  // Qué pasó con la última decisión aprobada, para que aprobar nunca parezca no hacer nada.
  const [notice, setNotice] = useState<{ text: string; worked: boolean } | null>(null);

  const names = Object.fromEntries(contexts.map((c) => [c.project.slug, c.project.name]));
  const hasFolder = Object.fromEntries(contexts.map((c) => [c.project.slug, Boolean(c.project.local_path)]));

  const load = useCallback(async () => {
    try {
      setData(await invoke<Payload>("list_decisions"));
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  const pending = data?.decisions.filter((d) => d.status === "pendiente") ?? [];
  const closed = data?.decisions.filter((d) => d.status !== "pendiente") ?? [];
  const current = data?.decisions.find((d) => d.id === selected) ?? pending[0] ?? null;

  // Al cambiar de decisión, el borrador parte de la propuesta del agente.
  useEffect(() => setDraft(current?.proposal ?? ""), [current?.id, current?.proposal]);

  async function act(kind: "ronda" | "decision", task: () => Promise<unknown>, decided?: number) {
    setBusy(kind);
    setError(null);
    setNotice(null);
    try {
      await task();
      setSelected(null);
      await Promise.all([load(), refresh()]);
      if (decided) {
        const fresh = await invoke<Payload>("list_decisions");
        const result = fresh.decisions.find((d) => d.id === decided)?.result ?? "";
        const worked = result.startsWith("Mejora");
        if (result) {
          setNotice({
            worked,
            text: worked ? `${result}. El agente la trabaja ahora, o apenas termine la que tiene en curso.` : `${result}.`,
          });
        }
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Decisiones</h1>
          <p>
            Lo que el agente encontró en su ronda y espera tu visto bueno. Nada se ejecuta hasta que apruebas, y puedes editar la
            propuesta antes.
          </p>
        </div>
        <div className="actions">
          {data?.round.at && <span className="stamp">Última ronda {ago(data.round.at)}</span>}
          <button className="btn primary" disabled={busy !== null} onClick={() => act("ronda", () => invoke("run_round"))}>
            {busy === "ronda" ? "El agente está revisando…" : "Pedir ronda ahora"}
          </button>
        </div>
      </header>

      {error && <div className="banner">{error}</div>}
      {notice && (
        <div className="banner ok">
          {notice.text}{" "}
          {notice.worked && (
            <button className="link" onClick={() => go("mejoras")}>
              Ver su avance en Mejoras →
            </button>
          )}
        </div>
      )}

      {data?.telegram.configured && (
        <p className="stamp telegram-line">
          {data.telegram.decisions
            ? "Las decisiones nuevas también te llegan por Telegram, con botones para aprobarlas o descartarlas. Escríbele /decisiones al bot para ver las pendientes."
            : "Las decisiones no te llegan por Telegram; solo los avisos graves."}{" "}
          <button
            className="link"
            disabled={busy !== null}
            onClick={() => act("decision", () => invoke("telegram_decisions", { on: !data.telegram.decisions }))}
          >
            {data.telegram.decisions ? "Dejar de mandarlas" : "Mandarlas con botones"}
          </button>
        </p>
      )}

      {data?.round.summary && (
        <section className="panel">
          <header className="panel-head">
            <h2>Resumen de la última ronda</h2>
            <div className="panel-aside">{data.round.auto ? "Ronda diaria activada" : "Ronda diaria desactivada"}</div>
          </header>
          <div className="panel-body round-summary">
            <Rich text={data.round.summary} />
          </div>
        </section>
      )}

      <div className="decisions">
        <section className="panel decision-list">
          <header className="panel-head">
            <h2>Pendientes</h2>
            <div className="panel-aside">
              <span className={pending.length ? "count hot" : "count"}>{pending.length}</span>
            </div>
          </header>
          {pending.length ? (
            pending.map((d) => (
              <button
                key={d.id}
                className={current?.id === d.id ? "decision-item active" : "decision-item"}
                onClick={() => setSelected(d.id)}
              >
                <span className="decision-title">{d.title}</span>
                <span className="stamp">
                  {names[d.slug] ?? d.slug} · {d.kind} · {ago(d.created_at)}
                </span>
              </button>
            ))
          ) : (
            <div className="panel-body">
              <Empty>Nada espera tu decisión.</Empty>
            </div>
          )}
          {closed.length > 0 && (
            <>
              <header className="panel-head sub-head">
                <h2>Resueltas</h2>
              </header>
              {closed.slice(0, 12).map((d) => (
                <button
                  key={d.id}
                  className={current?.id === d.id ? "decision-item active done" : "decision-item done"}
                  onClick={() => setSelected(d.id)}
                >
                  <span className="decision-title">{d.title}</span>
                  <span className="stamp">
                    {names[d.slug] ?? d.slug} · {d.status}
                  </span>
                </button>
              ))}
            </>
          )}
        </section>

        <section className="panel decision-detail">
          {current ? (
            <>
              <div className="decision-tags">
                <button className="link plain project-chip" onClick={() => openProject(current.slug)}>
                  {names[current.slug] ?? current.slug}
                </button>
                <Tag>{current.kind}</Tag>
                <Tag tone={current.action === "ninguna" ? undefined : "almenara"}>{ACTION_LABEL[current.action]}</Tag>
                <span className="stamp">{ago(current.created_at)}</span>
              </div>
              <h2 className="decision-heading">{current.title}</h2>

              <div className="field-label">Lo que se observó</div>
              <div className="quote">
                <Rich text={current.context || "Sin detalle."} />
              </div>

              <div className="field-label">Propuesta del agente</div>
              {current.status === "pendiente" ? (
                <textarea value={draft} onChange={(e) => setDraft(e.target.value)} rows={9} />
              ) : (
                <div className="quote">
                  <Rich text={current.proposal} />
                </div>
              )}

              {current.status === "pendiente" ? (
                <footer className="decision-foot">
                  <span className="decision-effect">{effect(current, hasFolder[current.slug])}</span>
                  <div className="actions">
                    <button
                      className="btn"
                      disabled={busy !== null}
                      onClick={() => act("decision", () => invoke("discard_decision", { id: current.id }))}
                    >
                      Descartar
                    </button>
                    {/* Una informativa se puede dar por vista sin que nadie la trabaje. */}
                    {current.action === "ninguna" && (
                      <button
                        className="btn"
                        disabled={busy !== null}
                        onClick={() =>
                          act(
                            "decision",
                            () => invoke("approve_decision", { id: current.id, proposal: draft, work: false }),
                            current.id,
                          )
                        }
                      >
                        Dar por vista
                      </button>
                    )}
                    {(current.action !== "ninguna" || hasFolder[current.slug]) && (
                      <button
                        className="btn primary"
                        disabled={busy !== null || !draft.trim()}
                        onClick={() =>
                          act(
                            "decision",
                            () => invoke("approve_decision", { id: current.id, proposal: draft, work: true }),
                            current.id,
                          )
                        }
                      >
                        {busy === "decision"
                          ? "Aplicando…"
                          : current.action === "ninguna"
                            ? "Aprobar y resolver"
                            : current.action === "actualizar"
                              ? "Poner al día"
                              : "Aprobar"}
                      </button>
                    )}
                  </div>
                </footer>
              ) : (
                <footer className="decision-foot">
                  <span className="decision-effect">{current.result}</span>
                </footer>
              )}
            </>
          ) : (
            <div className="panel-body">
              <Empty>Elige una decisión para ver su detalle, o pide una ronda al agente.</Empty>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
