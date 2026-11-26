import type { Console } from "../App";
import { type Context, KIND_LABEL, ago, alertsOf, api, parseCommit, worst } from "../api";
import { Empty, Panel, Pip, StatusTag } from "../ui";

/** Tabla del portafolio: un proyecto por fila, con lo esencial de su última ronda. */
export function Portfolio({ contexts, openProject, go }: Pick<Console, "contexts" | "openProject"> & { go?: Console["go"] }) {
  if (!contexts.length) {
    return (
      <Empty>
        Aún no hay proyectos.{" "}
        {go ? (
          <button className="link" onClick={() => go("bienvenida")}>
            Elige tus carpetas y regístralos
          </button>
        ) : (
          "Elige tus carpetas en Proyectos → Buscar proyectos"
        )}
        , o desde la terminal con <code>atalaya roots add</code> y <code>atalaya scan --add</code>.
      </Empty>
    );
  }
  return (
    <table className="table">
      <thead>
        <tr>
          <th>Proyecto</th>
          <th>Estado</th>
          <th>Sitio</th>
          <th>Último despliegue</th>
          <th>Último cambio</th>
          <th className="num">Alertas</th>
          <th />
        </tr>
      </thead>
      <tbody>
        {contexts.map((context) => (
          <PortfolioRow key={context.project.slug} context={context} onOpen={openProject} />
        ))}
      </tbody>
    </table>
  );
}

function PortfolioRow({ context: { project, live, checks }, onOpen }: { context: Context; onOpen: (slug: string) => void }) {
  const status = checks.length ? worst(checks) : undefined;
  const find = (kind: string) => checks.find((c) => c.kind === kind);
  const site = find("sitio");
  const deploy = find("despliegue");
  const remote = (find("github")?.details?.commits as { fecha: string }[] | undefined)?.[0];
  const last = live?.git?.recent_commits[0] ? parseCommit(live.git.recent_commits[0]) : remote ? { date: remote.fecha } : null;
  const alerts = checks.filter((c) => c.status !== "ok" && !c.acked).length;
  const ms = site?.details?.ms;

  return (
    <tr className="row-link" onClick={() => onOpen(project.slug)}>
      <td>
        <span className="cell-name">
          <Pip status={status} />
          {project.name}
        </span>
      </td>
      <td>
        <StatusTag status={status} />
      </td>
      <td className="mono">
        {!site ? "—" : site.status === "ok" ? (typeof ms === "number" ? `${ms} ms` : "en línea") : <span className="brasa">{site.summary}</span>}
      </td>
      <td className="clip">
        {deploy ? (
          <span className={deploy.status === "ok" ? "" : "almenara"}>{deploy.summary.replace(/^Último despliegue correcto: /, "")}</span>
        ) : (
          <span className="polvo">—</span>
        )}
      </td>
      <td className="mono">{last?.date ?? "—"}</td>
      <td className="num mono">{alerts ? <span className="almenara">{alerts}</span> : <span className="polvo">0</span>}</td>
      <td className="go">Examinar →</td>
    </tr>
  );
}

export function Vigia({ contexts, refresh, openProject, go }: Console) {
  const hasFolder = Object.fromEntries(contexts.map((c) => [c.project.slug, Boolean(c.project.local_path)]));
  /** Pone a Claude a resolver la alerta; queda en la cola de Mejoras. */
  const resolve = async (slug: string, kind: string) => {
    await api.resolveAlert(slug, kind);
    await refresh();
  };
  const alerts = alertsOf(contexts);
  const open = alerts.filter((a) => !a.acked);
  const allChecks = contexts.flatMap((c) => c.checks);
  const lastRound = allChecks.map((c) => c.checked_at).sort().pop();

  const acknowledge = async (slug: string, kind: string) => {
    await api.ackAlert(slug, kind, true);
    await refresh();
  };

  return (
    <div className="stack">
      <div className="split">
        <Panel
          title={
            <>
              <Pip status={open.length ? (open.some((a) => a.status === "falla") ? "falla" : "aviso") : "ok"} /> Última ronda
            </>
          }
          aside={lastRound ? ago(lastRound) : "sin datos"}
        >
          {contexts.length ? (
            <ul className="round">
              {contexts.map(({ project, checks }) => {
                const bad = checks.filter((c) => c.status !== "ok");
                const status = checks.length ? worst(checks) : undefined;
                return (
                  <li key={project.slug}>
                    <Pip status={status} />
                    <span>
                      <b>{project.name}:</b>{" "}
                      {!checks.length
                        ? "aún sin revisar."
                        : bad.length
                          ? bad.map((c) => `${KIND_LABEL[c.kind] ?? c.kind}: ${c.summary.toLowerCase()}`).join("; ") + "."
                          : `${checks.length} revisiones en orden.`}
                    </span>
                  </li>
                );
              })}
            </ul>
          ) : (
            <Empty>Sin proyectos registrados.</Empty>
          )}
        </Panel>

        <Panel
          title={
            <>
              <Pip status={open.length ? "aviso" : "ok"} /> Alertas abiertas
            </>
          }
          aside={
            <button className="link" onClick={() => go("alertas")}>
              {open.length} {open.length === 1 ? "activa" : "activas"} →
            </button>
          }
        >
          {open.length ? (
            <ul className="alert-list">
              {open.slice(0, 5).map((a) => (
                <li key={a.slug + a.kind}>
                  <div>
                    <div className="alert-top">
                      <span className={a.status === "falla" ? "sev brasa" : "sev almenara"}>
                        {a.status === "falla" ? "Falla" : "Atención"}
                      </span>
                      <button className="link plain" onClick={() => openProject(a.slug)}>
                        {a.project.name}
                      </button>
                      <span className="polvo">· {KIND_LABEL[a.kind] ?? a.kind}</span>
                    </div>
                    <div className="alert-text">{a.summary}</div>
                    <div className="stamp">{ago(a.checked_at)}</div>
                  </div>
                  <button
                    className={hasFolder[a.slug] ? "btn primary" : "btn"}
                    onClick={() => (hasFolder[a.slug] ? resolve(a.slug, a.kind) : acknowledge(a.slug, a.kind))}
                  >
                    {hasFolder[a.slug] ? "Resolver" : "Silenciar"}
                  </button>
                </li>
              ))}
            </ul>
          ) : (
            <Empty>Nada requiere tu atención.</Empty>
          )}
        </Panel>
      </div>

      <Panel title="Portafolio de proyectos" aside={`${contexts.length} ${contexts.length === 1 ? "nodo" : "nodos"}`} flush>
        <Portfolio contexts={contexts} openProject={openProject} go={go} />
      </Panel>
    </div>
  );
}
