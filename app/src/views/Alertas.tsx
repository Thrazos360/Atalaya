import { useState } from "react";
import type { Console } from "../App";
import { KIND_LABEL, ago, alertsOf, api, detailLine, everyLabel, localTime } from "../api";
import { Empty, Panel, Tag } from "../ui";

export function Alertas({ contexts, improvements, system, refresh, openProject, go }: Console) {
  const [tab, setTab] = useState<"abiertas" | "reconocidas">("abiertas");
  const alerts = alertsOf(contexts);
  const open = alerts.filter((a) => !a.acked);
  const acked = alerts.filter((a) => a.acked);
  const shown = tab === "abiertas" ? open : acked;

  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const run = async (key: string, task: () => Promise<unknown>) => {
    setBusy(key);
    setError(null);
    try {
      await task();
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };
  const setAck = (slug: string, kind: string, value: boolean) =>
    run(slug + kind, () => api.ackAlert(slug, kind, value));

  /** La mejora que ya está resolviendo esta alerta, si la hay. */
  const working = (slug: string, kind: string) =>
    improvements.find(
      (i) => i.slug === slug && i.title === `Resolver alerta de ${kind}` && i.status !== "hecha" && i.status !== "descartada",
    );
  const hasFolder = Object.fromEntries(contexts.map((c) => [c.project.slug, Boolean(c.project.local_path)]));

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Alertas</h1>
          <p>
            Todo lo que las rondas encontraron fuera de lo normal. “Resolver” pone a Claude a arreglarla; “Silenciar” solo la
            deja de contar como abierta.
          </p>
        </div>
      </header>

      {error && <div className="banner">{error}</div>}

      <div className="with-aside">
        <div className="stack">
          <div className="tabs">
            <button className={tab === "abiertas" ? "tab active" : "tab"} onClick={() => setTab("abiertas")}>
              Abiertas <span className="count">{open.length}</span>
            </button>
            <button className={tab === "reconocidas" ? "tab active" : "tab"} onClick={() => setTab("reconocidas")}>
En resolución o silenciadas <span className="count">{acked.length}</span>
            </button>
          </div>

          <section className="panel">
            {shown.length ? (
              <table className="table tall">
                <thead>
                  <tr>
                    <th>Sev</th>
                    <th>Proyecto</th>
                    <th>Diagnóstico</th>
                    <th className="num">Tiempo / acciones</th>
                  </tr>
                </thead>
                <tbody>
                  {shown.map((a) => {
                    const detail = detailLine(a.details);
                    return (
                      <tr key={a.slug + a.kind} className={a.status === "falla" && !a.acked ? "hot-row" : ""}>
                        <td>
                          <span className={a.status === "falla" ? "sev brasa" : "sev almenara"}>
                            {a.status === "falla" ? "Falla" : "Atn"}
                          </span>
                        </td>
                        <td>
                          <button className="link plain strong" onClick={() => openProject(a.slug)}>
                            {a.project.name}
                          </button>
                          <div className="stamp">{KIND_LABEL[a.kind] ?? a.kind}</div>
                        </td>
                        <td>
                          <div className="diag">{a.summary}</div>
                          {detail && <div className="readout">{detail}</div>}
                        </td>
                        <td className="num">
                          <div className={a.acked ? "stamp" : "stamp almenara"}>{ago(a.checked_at)}</div>
                          <div className="stamp">{localTime(a.checked_at)}</div>
                          {(() => {
                            const job = working(a.slug, a.kind);
                            const key = a.slug + a.kind;
                            return (
                              <div className="row-actions">
                                {job ? (
                                  <button className="link" onClick={() => go("mejoras")}>
                                    En resolución: mejora #{job.id} →
                                  </button>
                                ) : (
                                  hasFolder[a.slug] && (
                                    <button
                                      className="btn primary"
                                      disabled={busy !== null}
                                      onClick={() => run(key, () => api.resolveAlert(a.slug, a.kind))}
                                    >
                                      {busy === key ? "Enviando…" : "Resolver"}
                                    </button>
                                  )
                                )}
                                <button className="btn" disabled={busy !== null} onClick={() => setAck(a.slug, a.kind, !a.acked)}>
                                  {a.acked ? "Reabrir" : "Silenciar"}
                                </button>
                              </div>
                            );
                          })()}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            ) : (
              <div className="panel-body">
                <Empty>{tab === "abiertas" ? "Sin alertas abiertas." : "Ninguna alerta en resolución ni silenciada."}</Empty>
              </div>
            )}
          </section>
        </div>

        <Panel title="Rondas activas" aside={<Tag tone="almenara">{system?.rounds.length ?? 0}</Tag>}>
          <ul className="rules">
            {system?.rounds.map((round) => (
              <li key={round.kind}>
                <div className="rule-name">{KIND_LABEL[round.kind] ?? round.kind}</div>
                <div className="readout">{RULES[round.kind] ?? "revisión periódica"}</div>
                <div className="stamp">{everyLabel(round.every_seconds)}</div>
              </li>
            ))}
          </ul>
          <p className="footnote">
            El vigía repite cada revisión cuando le toca y avisa por notificación y correo solo cuando el estado cambia.
          </p>
        </Panel>
      </div>
    </div>
  );
}

/** Qué dispara una alerta en cada tipo de revisión. */
const RULES: Record<string, string> = {
  sitio: "falla si responde ≥ 400 o no responde; aviso si tarda > 5 s",
  certificado: "falla si el certificado vence en menos de 3 días; aviso en menos de 3 semanas",
  migraciones: "aviso si hay migraciones del repo sin aplicar en producción",
  respaldo: "falla si el respaldo diario de la base no se pudo hacer",
  firebase: "falla si el proyecto no está activo o las reglas permiten escribir a cualquiera; aviso si difieren del repo",
  github: "falla si las comprobaciones del último commit fallaron",
  despliegue: "falla si el último despliegue de producción falló",
  supabase: "falla si la base está pausada o hay errores de seguridad",
  git: "aviso si hay archivos sin confirmar o commits sin subir",
  dependencias: "falla con vulnerabilidades graves; aviso con el resto",
};
