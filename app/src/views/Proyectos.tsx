import { useCallback, useEffect, useState } from "react";
import { AdminPanel } from "./Administracion";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { Console } from "../App";
import {
  type Connections,
  type Context,
  type DayStat,
  type Flags,
  KIND_LABEL,
  type Link,
  type Repo,
  api,
  localTime,
  parseCommit,
  worst,
} from "../api";
import type { View } from "../App";
import { Empty, Panel, Pip, StatusTag, Tag } from "../ui";
import { Portfolio } from "./Vigia";

export function Proyectos(props: Console & { selected: string | null; onBack: () => void }) {
  const current = props.contexts.find((c) => c.project.slug === props.selected);
  if (current) return <Detail {...props} context={current} />;
  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Proyectos</h1>
          <p>Cada proyecto con su estado en vivo. Abre uno para ver su vigilancia, sus interruptores y su contexto.</p>
        </div>
        <button className="btn" onClick={() => props.go("bienvenida")}>
          Buscar proyectos
        </button>
      </header>
      <Panel title="Portafolio de proyectos" aside={`${props.contexts.length} nodos`} flush>
        <Portfolio contexts={props.contexts} openProject={props.openProject} go={props.go} />
      </Panel>
      <Repos {...props} />
    </div>
  );
}

/** Repos de GitHub de la cuenta: vigilar los que faltan y crear nuevos. */
function Repos({ contexts, refresh }: Console) {
  const [repos, setRepos] = useState<Repo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [creating, setCreating] = useState(false);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [isPublic, setIsPublic] = useState(false);

  const load = useCallback(async () => {
    try {
      setRepos(await api.githubRepos());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  const act = async (task: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await task();
      setError(null);
      await Promise.all([load(), refresh()]);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  async function create(event: React.FormEvent) {
    event.preventDefault();
    if (await act(() => api.githubCreate(name.trim(), desc.trim(), !isPublic))) {
      setName("");
      setDesc("");
      setCreating(false);
    }
  }

  const watched = new Set(contexts.map((c) => c.project.repo?.toLowerCase()).filter(Boolean));

  return (
    <Panel
      title="Repositorios de GitHub"
      aside={
        <button className="link" onClick={() => setCreating((v) => !v)}>
          {creating ? "Cancelar" : "+ Crear repo"}
        </button>
      }
      flush
    >
      {error && <p className="error-note pad">{error}</p>}
      {creating && (
        <form className="form bordered" onSubmit={create}>
          <div className="form-row">
            <label>
              Nombre
              <input
                className="mono"
                value={name}
                onChange={(e) => setName(e.target.value.replace(/[^A-Za-z0-9._-]/g, "-"))}
                placeholder="mi-proyecto"
                spellCheck={false}
                autoFocus
              />
            </label>
            <label className="grow">
              Descripción
              <input value={desc} onChange={(e) => setDesc(e.target.value)} placeholder="Para qué es" />
            </label>
          </div>
          <div className="form-actions spread">
            <label className="check">
              <input type="checkbox" checked={isPublic} onChange={(e) => setIsPublic(e.target.checked)} />
              Público (por defecto se crea privado)
            </label>
            <button className="btn primary" disabled={!name.trim() || busy}>
              {busy ? "Creando…" : "Crear en GitHub"}
            </button>
          </div>
        </form>
      )}
      {repos === null ? (
        !error && (
          <div className="panel-body">
            <Empty>Leyendo de GitHub…</Empty>
          </div>
        )
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>Repo</th>
              <th>Visibilidad</th>
              <th>Último cambio</th>
              <th>Descripción</th>
              <th className="num">Vigilancia</th>
            </tr>
          </thead>
          <tbody>
            {repos.map((repo) => (
              <tr key={repo.full_name}>
                <td>
                  <button className="link plain strong" onClick={() => openUrl(repo.url)}>
                    {repo.full_name}
                  </button>
                </td>
                <td>
                  <Tag>{repo.private ? "Privado" : "Público"}</Tag>
                </td>
                <td className="mono">{repo.pushed_at?.slice(0, 10) ?? "—"}</td>
                <td className="clip">{repo.description ?? <span className="polvo">—</span>}</td>
                <td className="num">
                  {watched.has(repo.full_name.toLowerCase()) ? (
                    <Tag tone="liquen">Vigilado</Tag>
                  ) : (
                    <button
                      className="btn"
                      disabled={busy}
                      onClick={() => act(() => api.githubWatch(repo.full_name, repo.homepage))}
                    >
                      Vigilar
                    </button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}

function Detail({
  context: { project, live, checks },
  improvements,
  refresh,
  onBack,
  go,
}: Console & { context: Context; onBack: () => void }) {
  const [doc, setDoc] = useState(0);
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState<string | null>(null);
  useEffect(() => setDoc(0), [project.slug]);

  async function runChecks() {
    setChecking(true);
    setCheckError(null);
    try {
      await api.runChecks(project.slug);
      await refresh();
    } catch (e) {
      setCheckError(String(e));
    } finally {
      setChecking(false);
    }
  }

  const git = live?.git;
  // Sin carpeta local, los cambios recientes vienen de la revisión de GitHub.
  const remote = (checks.find((c) => c.kind === "github")?.details?.commits ?? []) as {
    sha: string;
    fecha: string;
    mensaje: string;
  }[];
  const commits = git
    ? git.recent_commits.map(parseCommit)
    : remote.map((c) => ({ hash: c.sha, date: c.fecha, message: c.mensaje }));
  const last = commits[0] ?? null;
  const deps = Object.entries(live?.stack?.dependencies ?? {});
  const devDeps = Object.entries(live?.stack?.dev_dependencies ?? {});
  const docs = live?.docs ?? [];
  const status = checks.length ? worst(checks) : undefined;
  const openAlerts = checks.filter((c) => c.status !== "ok" && !c.acked).length;
  const pending = improvements.filter((i) => i.slug === project.slug && i.status !== "hecha" && i.status !== "descartada");
  const services = [
    ["Vercel", live?.services.vercel_project_id],
    ["Supabase", live?.services.supabase_project_ref],
    ["Firebase", live?.services.firebase_project],
  ].filter(([, id]) => id) as [string, string][];

  return (
    <div className="stack">
      <header className="crumbs">
        <div className="crumb-path">
          <button className="link" onClick={onBack}>
            Proyectos
          </button>
          <span className="polvo">/</span>
          <span className="crumb-name">{project.name}</span>
          <StatusTag status={status} />
          {git && <Tag>{git.branch}</Tag>}
          {!project.local_path && <Tag>Solo remoto</Tag>}
        </div>
        <div className="actions">
          {!project.local_path && project.repo && (
            <button
              className="btn"
              disabled={checking}
              title="Trae el código a este Mac para tener interruptores, dependencias y agente"
              onClick={async () => {
                setChecking(true);
                setCheckError(null);
                try {
                  await api.cloneProject(project.slug);
                  await refresh();
                } catch (e) {
                  setCheckError(String(e));
                } finally {
                  setChecking(false);
                }
              }}
            >
              Clonar aquí
            </button>
          )}
          {checks.some((c) => c.kind === "supabase" || c.kind === "firebase") && (
            <button
              className="btn"
              disabled={checking}
              onClick={async () => {
                setChecking(true);
                setCheckError(null);
                try {
                  await api.backupProject(project.slug);
                  await refresh();
                } catch (e) {
                  setCheckError(String(e));
                } finally {
                  setChecking(false);
                }
              }}
            >
              Respaldar ahora
            </button>
          )}
          <button
            className="btn danger"
            title="Deja de vigilarlo. No toca su carpeta ni su repo."
            onClick={async () => {
              await api.removeProject(project.slug);
              onBack();
              await refresh();
            }}
          >
            Quitar
          </button>
          <button className="btn" onClick={runChecks} disabled={checking}>
            {checking ? "Revisando…" : "Revisar ahora"}
          </button>
          {project.repo && (
            <button className="btn" onClick={() => openUrl(`https://github.com/${project.repo}`)}>
              GitHub ↗
            </button>
          )}
          {project.production_url && (
            <button className="btn primary" onClick={() => openUrl(project.production_url!)}>
              Abrir sitio ↗
            </button>
          )}
        </div>
      </header>

      {project.description && <p className="lede">{project.description}</p>}
      {live?.error && <div className="banner">{live.error}</div>}
      {checkError && <div className="banner">{checkError}</div>}

      <div className="tiles">
        <Tile label="Alertas abiertas" value={String(openAlerts)} tone={openAlerts ? "almenara" : undefined} />
        <Tile
          label="Sin confirmar"
          value={git ? String(git.uncommitted_files) : "—"}
          note={
            git ? (git.uncommitted_files ? (git.uncommitted_files === 1 ? "archivo" : "archivos") : "todo confirmado") : undefined
          }
          tone={git?.uncommitted_files ? "almenara" : undefined}
        />
        <Tile label="Último cambio" value={last?.date ?? "—"} small />
        <Tile label="Dependencias" value={live?.stack ? String(deps.length + devDeps.length) : "—"} />
        <Tile label="Mejoras pendientes" value={String(pending.length)} tone={pending.length ? "almenara" : undefined} />
      </div>

      <div className="split">
        <Panel title="Vigilancia" aside={checks.length ? `${checks.length} revisiones` : undefined} flush>
          {checks.length ? (
            <table className="table">
              <tbody>
                {checks.map((c) => (
                  <tr key={c.kind}>
                    <td className="shrink">
                      <span className="cell-name">
                        <Pip status={c.status} />
                        {KIND_LABEL[c.kind] ?? c.kind}
                      </span>
                    </td>
                    <td>
                      {c.summary}
                      {c.acked && <span className="polvo"> · reconocida</span>}
                    </td>
                    <td className="num stamp shrink">{localTime(c.checked_at)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <div className="panel-body">
              <Empty>Aún no se ha revisado este proyecto.</Empty>
            </div>
          )}
        </Panel>

        <FlagsPanel slug={project.slug} />
      </div>

      <AdminPanel slug={project.slug} />

      <Trend slug={project.slug} />

      <div className="split">
        <Panel title="Últimos cambios" aside={project.repo ?? undefined} flush>
          {commits.length ? (
            <table className="table">
              <tbody>
                {commits.map((c) => (
                  <tr key={c.hash}>
                    <td className="stamp shrink">{c.date}</td>
                    <td>{c.message}</td>
                    <td className="num stamp shrink">{c.hash}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <div className="panel-body">
              <Empty>Sin historial.</Empty>
            </div>
          )}
        </Panel>

        <div className="stack">
          <Panel title="Servicios conectados">
            {services.length ? (
              <ul className="kv">
                {services.map(([name, id]) => (
                  <li key={name}>
                    <span>{name}</span>
                    <span className="mono selectable">{id}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <Empty>No se detectó ningún servicio.</Empty>
            )}
            <p className="footnote selectable">
              {project.local_path ?? "Sin carpeta en esta máquina: se vigila desde GitHub y sus servicios."}
            </p>
          </Panel>

          <LinksPanel slug={project.slug} go={go} />

          <Panel
            title="Mejoras pendientes"
            aside={
              <button className="link" onClick={() => go("mejoras")}>
                Ver todas →
              </button>
            }
          >
            {pending.length ? (
              <ul className="kv">
                {pending.map((i) => (
                  <li key={i.id}>
                    <span>{i.title}</span>
                    <Tag tone={i.status === "propuesta" ? "almenara" : "liquen"}>{i.status.replace("_", " ")}</Tag>
                  </li>
                ))}
              </ul>
            ) : (
              <Empty>Sin mejoras pendientes.</Empty>
            )}
          </Panel>
        </div>
      </div>

      {live?.stack && (
        <Panel title="Dependencias" aside={`${deps.length} de producción · ${devDeps.length} de desarrollo`}>
          <DepGrid items={deps} />
          {devDeps.length > 0 && (
            <>
              <h3 className="sub">De desarrollo</h3>
              <DepGrid items={devDeps} />
            </>
          )}
        </Panel>
      )}

      {docs.length > 0 && (
        <Panel
          title="Documentos del repo"
          aside={
            <div className="tabs inline">
              {docs.map((d, i) => (
                <button key={d.file} className={i === doc ? "tab active" : "tab"} onClick={() => setDoc(i)}>
                  {d.file}
                </button>
              ))}
            </div>
          }
        >
          <pre className="doc">
            {docs[doc]?.content}
            {docs[doc]?.truncated && "\n\n… (recortado)"}
          </pre>
        </Panel>
      )}
    </div>
  );
}

/** Disponibilidad y tiempo de respuesta del sitio, día por día. */
function Trend({ slug }: { slug: string }) {
  const [days, setDays] = useState<DayStat[]>([]);
  useEffect(() => {
    setDays([]);
    api
      .siteHistory(slug)
      .then(setDays)
      .catch(() => {});
  }, [slug]);
  if (!days.length) return null;

  const total = days.reduce((n, d) => n + d.total, 0);
  const ok = days.reduce((n, d) => n + d.ok, 0);
  const uptime = total ? (ok / total) * 100 : 100;
  const timed = days.filter((d) => d.ms !== null);
  const average = timed.length ? timed.reduce((n, d) => n + (d.ms ?? 0), 0) / timed.length : 0;
  const peak = Math.max(...timed.map((d) => d.ms ?? 0), 1);

  return (
    <Panel
      title="Tendencia del sitio"
      aside={`${uptime.toLocaleString("es", { maximumFractionDigits: 2 })} % disponible · ${Math.round(average)} ms en promedio · ${days.length} días`}
    >
      <div className="bars">
        {days.map((d) => {
          const failed = d.total - d.ok;
          return (
            <div
              className="bar"
              key={d.day}
              title={`${d.day}: ${d.ms === null ? "sin respuesta" : `${Math.round(d.ms)} ms`}, ${failed ? `${failed} de ${d.total} revisiones con falla` : "sin fallas"}`}
            >
              <div
                className={failed ? "bar-fill brasa" : "bar-fill liquen"}
                style={{ height: `${Math.max(3, ((d.ms ?? peak) / peak) * 100)}%` }}
              />
              <span className="stamp">{d.day.slice(8)}</span>
            </div>
          );
        })}
      </div>
      <p className="footnote">
        Cada barra es un día: su altura es el tiempo de respuesta promedio y se pone roja si alguna revisión falló. Se conservan
        30 días.
      </p>
    </Panel>
  );
}

function Tile({
  label,
  value,
  note,
  tone,
  small,
}: {
  label: string;
  value: string;
  note?: string;
  tone?: "almenara";
  small?: boolean;
}) {
  return (
    <div className="tile">
      <div className="tile-label">{label}</div>
      <div className={`tile-value ${tone ?? ""} ${small ? "small" : ""}`}>{value}</div>
      {note && <div className="stamp">{note}</div>}
    </div>
  );
}

function DepGrid({ items }: { items: [string, string][] }) {
  return (
    <div className="deps">
      {items.map(([name, version]) => (
        <div className="dep" key={name}>
          <span>{name}</span>
          <span className="stamp">{version}</span>
        </div>
      ))}
    </div>
  );
}

/** Interruptores de funciones de un proyecto. Cada cambio se guarda en Vercel al instante. */
function FlagsPanel({ slug }: { slug: string }) {
  const [flags, setFlags] = useState<Flags | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");

  const run = useCallback(
    async (command: string, args: Record<string, unknown> = {}) => {
      setBusy(true);
      setError(null);
      try {
        setFlags(await api.flags(command, slug, args));
        return true;
      } catch (e) {
        setError(String(e));
        return false;
      } finally {
        setBusy(false);
      }
    },
    [slug],
  );

  useEffect(() => {
    setFlags(null);
    setName("");
    setDesc("");
    run("list_flags");
  }, [run]);

  async function add(event: React.FormEvent) {
    event.preventDefault();
    if (await run("add_flag", { name: name.trim(), desc: desc.trim() })) {
      setName("");
      setDesc("");
    }
  }

  const entries = Object.entries(flags ?? {});
  const on = entries.filter(([, f]) => f.on).length;

  return (
    <Panel title="Interruptores" aside={flags ? `${on} de ${entries.length} prendidos` : undefined}>
      {error && <p className="error-note">{error}</p>}
      {flags === null ? (
        <Empty>{busy ? "Leyendo de Vercel…" : "No se pudieron leer."}</Empty>
      ) : entries.length ? (
        <ul className="flags">
          {entries.map(([flag, { on, desc, value }]) => (
            <li key={flag}>
              <button
                role="switch"
                aria-checked={on}
                aria-label={flag}
                className={on ? "switch on" : "switch"}
                disabled={busy}
                onClick={() => run("set_flag", { name: flag, on: !on })}
              />
              <div className="flag-text">
                <div className="flag-name">{flag}</div>
                {desc && <div className="flag-desc">{desc}</div>}
                <input
                  key={value ?? ""}
                  defaultValue={value ?? ""}
                  placeholder="Texto (opcional)"
                  disabled={busy}
                  onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
                  onBlur={(e) => {
                    const text = e.currentTarget.value.trim();
                    if (text !== (value ?? "")) run("set_flag_text", { name: flag, text });
                  }}
                />
              </div>
              <span className={on ? "flag-state liquen" : "flag-state"}>{on ? "Prendido" : "Apagado"}</span>
              <button
                className="icon-btn"
                title="Eliminar interruptor"
                disabled={busy}
                onClick={() => run("remove_flag", { name: flag })}
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <Empty>Este proyecto aún no tiene interruptores.</Empty>
      )}
      <form className="inline-form" onSubmit={add}>
        <input
          className="mono"
          value={name}
          onChange={(e) => setName(e.target.value.toLowerCase().replace(/[^a-z0-9_]/g, "_"))}
          placeholder="nombre_del_interruptor"
          spellCheck={false}
        />
        <input value={desc} onChange={(e) => setDesc(e.target.value)} placeholder="Qué hace" />
        <button className="btn" disabled={!name.trim() || busy}>
          Agregar
        </button>
      </form>
    </Panel>
  );
}

/** Lo propio del proyecto en cada proveedor al vincularlo. */
const LINK_FIELDS: Record<string, { key: string; label: string; placeholder: string }[]> = {
  supabase: [{ key: "ref", label: "Referencia del proyecto", placeholder: "abcdefghijklmnopqrst" }],
  vercel: [
    { key: "project_id", label: "ID del proyecto", placeholder: "prj_…" },
    { key: "team_id", label: "ID del equipo (si no es el de la conexión)", placeholder: "team_…" },
  ],
  netlify: [{ key: "site_id", label: "ID del sitio", placeholder: "Se ve en Site configuration → Site ID" }],
  gitlab: [{ key: "repo", label: "Repositorio", placeholder: "grupo/proyecto" }],
  gitea: [{ key: "repo", label: "Repositorio", placeholder: "dueño/repo" }],
  postgres: [{ key: "database", label: "Base (si no es la de la URL)", placeholder: "nombre de la base" }],
  mysql: [{ key: "database", label: "Base (si no es la de la URL)", placeholder: "nombre de la base" }],
};

const ROLE_KIND: Record<string, string> = { repo: "git", hosting: "hosting", base: "base", respaldo: "base" };

/** Qué conexiones usa este proyecto (repo, hosting, bases, respaldo) y con qué datos. */
function LinksPanel({ slug, go }: { slug: string; go: (view: View) => void }) {
  const [data, setData] = useState<Connections | null>(null);
  const [links, setLinks] = useState<Link[]>([]);
  const [adding, setAdding] = useState(false);
  const [role, setRole] = useState("base");
  const [connectionId, setConnectionId] = useState("");
  const [config, setConfig] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [all, mine] = await Promise.all([api.connections(), api.links(slug)]);
      setData(all);
      setLinks(mine);
    } catch (e) {
      setError(String(e));
    }
  }, [slug]);
  useEffect(() => {
    load();
  }, [load]);

  if (!data) return null;
  const byId = (id: string) => data.connections.find((c) => c.id === id);
  const roleLabel = (id: string) => data.roles.find((r) => r.id === id)?.label ?? id;
  const candidates = data.connections.filter((c) => c.kind === ROLE_KIND[role]);
  const chosen = byId(connectionId) ?? candidates[0];
  const fields = chosen ? (LINK_FIELDS[chosen.provider] ?? []) : [];

  const add = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!chosen) return;
    try {
      await api.setLink(slug, role, chosen.id, config);
      setAdding(false);
      setConfig({});
      setError(null);
      await load();
    } catch (err) {
      setError(String(err));
    }
  };

  return (
    <Panel
      title="Conexiones del proyecto"
      aside={
        !adding && (
          <button className="link" onClick={() => setAdding(true)}>
            Vincular
          </button>
        )
      }
    >
      {links.length ? (
        <ul className="kv">
          {links.map((l) => {
            const c = byId(l.connection_id);
            const extra = Object.values(l.config ?? {})
              .filter(Boolean)
              .join(" · ");
            return (
              <li key={`${l.role}-${l.connection_id}`}>
                <span>{roleLabel(l.role)}</span>
                <span className="cell-name">
                  <span className="mono selectable">
                    {c?.name ?? l.connection_id}
                    {extra ? ` · ${extra}` : ""}
                  </span>
                  <button
                    className="link"
                    title="Quitar el vínculo (la conexión sigue existiendo)"
                    onClick={async () => {
                      await api.unlink(slug, l.role, l.connection_id).catch((err) => setError(String(err)));
                      await load();
                    }}
                  >
                    Quitar
                  </button>
                </span>
              </li>
            );
          })}
        </ul>
      ) : (
        !adding && <Empty>Sin vínculos: Atalaya usa lo que detecta en el repo y las conexiones predeterminadas.</Empty>
      )}
      {adding &&
        (data.connections.length === 0 ? (
          <Empty>
            Primero agrega una conexión en{" "}
            <button className="link" onClick={() => go("conexiones")}>
              Conexiones
            </button>
            .
          </Empty>
        ) : (
          <form className="form" onSubmit={add}>
            <div className="form-row">
              <label>
                Rol
                <select
                  value={role}
                  onChange={(e) => {
                    setRole(e.target.value);
                    setConnectionId("");
                    setConfig({});
                  }}
                >
                  {data.roles.map((r) => (
                    <option key={r.id} value={r.id}>
                      {r.label}
                    </option>
                  ))}
                </select>
              </label>
              <label className="grow">
                Conexión
                <select
                  value={chosen?.id ?? ""}
                  onChange={(e) => {
                    setConnectionId(e.target.value);
                    setConfig({});
                  }}
                >
                  {candidates.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            {!candidates.length && <p className="footnote">No hay conexiones de este tipo todavía.</p>}
            {fields.map((f) => (
              <label key={f.key}>
                {f.label}
                <input
                  value={config[f.key] ?? ""}
                  onChange={(e) => setConfig({ ...config, [f.key]: e.target.value })}
                  placeholder={f.placeholder}
                  spellCheck={false}
                />
              </label>
            ))}
            <div className="form-actions spread">
              <button type="button" className="btn" onClick={() => setAdding(false)}>
                Cancelar
              </button>
              <button className="btn primary" disabled={!chosen}>
                Vincular
              </button>
            </div>
          </form>
        ))}
      {error && <div className="error-note">{error}</div>}
    </Panel>
  );
}
