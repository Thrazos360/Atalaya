import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { Console } from "../App";
import { Empty, Panel, Tag } from "../ui";

type Suggestion = { role: string; connection_id: string; connection: string; config: Record<string, string>; reason: string };
type Found = {
  path: string;
  name: string;
  slug: string;
  registered: string | null;
  remote: string | null;
  services: string[];
  suggestions: Suggestion[];
};

/** Carpetas elegidas, con el selector de macOS para agregar más. */
export function Carpetas({ onChange }: { onChange?: () => void }) {
  const [roots, setRoots] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setRoots(await invoke<string[]>("roots_list"));
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  const add = async () => {
    const picked = await open({ directory: true, multiple: true, title: "Elige dónde guardas tus proyectos" });
    const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
    try {
      for (const path of list) await invoke("roots_add", { path });
      setError(null);
    } catch (e) {
      setError(String(e));
    }
    await load();
    onChange?.();
  };

  return (
    <Panel
      title="Carpetas de proyectos"
      aside={
        <button className="btn primary" onClick={add}>
          Elegir carpeta
        </button>
      }
    >
      {roots.length ? (
        <ul className="kv">
          {roots.map((root) => (
            <li key={root}>
              <span className="mono selectable">{root}</span>
              <button
                className="link"
                title="Deja de buscar aquí. No toca la carpeta ni los proyectos ya registrados."
                onClick={async () => {
                  await invoke("roots_remove", { path: root }).catch((e) => setError(String(e)));
                  await load();
                  onChange?.();
                }}
              >
                Quitar de la lista
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <Empty>
          Elige la carpeta (o las carpetas) donde guardas tus proyectos. Atalaya busca ahí, hasta tres niveles, las carpetas
          que tienen un repositorio git.
        </Empty>
      )}
      {error && <div className="error-note">{error}</div>}
    </Panel>
  );
}

/** Lo que se encontró en las carpetas, para elegir qué registrar. */
export function Encontrados({ version, onRegistered }: { version: number; onRegistered: () => Promise<void> }) {
  const [found, setFound] = useState<Found[] | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [link, setLink] = useState(true);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const scan = useCallback(async () => {
    setFound(null);
    try {
      const list = await invoke<Found[]>("scan_projects");
      setFound(list);
      setChosen(new Set(list.filter((f) => !f.registered).map((f) => f.path)));
      setError(null);
    } catch (e) {
      setError(String(e));
      setFound([]);
    }
  }, []);
  useEffect(() => {
    scan();
  }, [scan, version]);

  const toggle = (path: string) => {
    const next = new Set(chosen);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    setChosen(next);
  };

  const register = async () => {
    setBusy(true);
    try {
      const r = await invoke<{ projects: number; links: number }>("register_projects", { paths: [...chosen], link });
      setNotice(
        `${r.projects} ${r.projects === 1 ? "proyecto registrado" : "proyectos registrados"}` +
          (r.links ? ` y ${r.links} ${r.links === 1 ? "vínculo creado" : "vínculos creados"}.` : "."),
      );
      await onRegistered();
      await scan();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const fresh = found?.filter((f) => !f.registered) ?? [];
  return (
    <Panel
      title="Proyectos encontrados"
      aside={
        <button className="link" onClick={scan}>
          Buscar de nuevo
        </button>
      }
      flush
    >
      {found === null ? (
        <div className="panel-body">
          <Empty>Buscando…</Empty>
        </div>
      ) : found.length === 0 ? (
        <div className="panel-body">
          <Empty>No hay proyectos en las carpetas elegidas todavía.</Empty>
        </div>
      ) : (
        <>
          <table className="table">
            <tbody>
              {found.map((f) => (
                <tr key={f.path}>
                  <td className="shrink">
                    <input
                      type="checkbox"
                      aria-label={`Registrar ${f.name}`}
                      checked={chosen.has(f.path)}
                      disabled={Boolean(f.registered)}
                      onChange={() => toggle(f.path)}
                    />
                  </td>
                  <td>
                    <div className="strong">{f.name}</div>
                    <div className="stamp selectable">{f.path}</div>
                  </td>
                  <td>
                    {f.services.map((s) => (
                      <Tag key={s}>{s}</Tag>
                    ))}
                    {f.suggestions.map((s) => (
                      <div key={`${s.role}-${s.connection_id}`} className="stamp">
                        Vincular {s.connection} ({s.reason})
                      </div>
                    ))}
                  </td>
                  <td className="shrink">{f.registered ? <Tag tone="liquen">Registrado</Tag> : <Tag>Nuevo</Tag>}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="form-actions spread pad">
            <label className="check stamp">
              <input type="checkbox" checked={link} onChange={(e) => setLink(e.target.checked)} /> Crear los vínculos propuestos
            </label>
            <button className="btn primary" disabled={busy || chosen.size === 0} onClick={register}>
              {busy ? "Registrando…" : `Registrar ${chosen.size || ""}`.trim()}
            </button>
          </div>
        </>
      )}
      {fresh.length === 0 && found && found.length > 0 && !notice && (
        <p className="footnote pad">Todo lo encontrado ya está registrado.</p>
      )}
      {notice && <p className="footnote pad liquen">{notice}</p>}
      {error && <div className="error-note">{error}</div>}
    </Panel>
  );
}

/** Primer uso: carpetas → proyectos → conexiones → IA. Las dos últimas se pueden saltar. */
export function Bienvenida({ contexts, refresh, go }: Console) {
  const [version, setVersion] = useState(0);
  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>{contexts.length ? "Buscar proyectos" : "Bienvenido a Atalaya"}</h1>
          <p>
            Atalaya vigila tus proyectos: sitios, despliegues, bases de datos, repos y dependencias, y te avisa cuando algo
            cambia. Empieza por decirle dónde están.
          </p>
        </div>
      </header>

      <ol className="steps">
        <li>
          <b>Carpetas.</b> Elige dónde guardas tus proyectos.
        </li>
        <li>
          <b>Proyectos.</b> Marca los que quieres vigilar y regístralos.
        </li>
        <li>
          <b>Conexiones</b> (puedes saltarlo). Repos, hosting y bases para revisar más que la carpeta.
        </li>
        <li>
          <b>IA</b> (puedes saltarlo). Con tu suscripción, modelos de este Mac o por API.
        </li>
      </ol>

      <Carpetas onChange={() => setVersion((v) => v + 1)} />
      <Encontrados version={version} onRegistered={refresh} />

      <div className="split">
        <Panel title="Conexiones">
          <p>
            GitHub, GitLab o Gitea; Vercel, Netlify o tu propio servidor; Supabase, Firebase, Postgres, MySQL o SQLite; correo
            y Telegram para los avisos.
          </p>
          <div className="form-actions">
            <button className="btn" onClick={() => go("conexiones")}>
              Ir a Conexiones
            </button>
          </div>
        </Panel>
        <Panel title="IA">
          <p>
            Con tu cuenta, sin llaves: Claude Code, Codex (ChatGPT), Gemini, Copilot, Cursor, Kimi, Qwen y más. También modelos
            de este Mac (Ollama, LM Studio, Jan) o cualquier API con llave.
          </p>
          <div className="form-actions">
            <button className="btn" onClick={() => go("ia")}>
              Conectar una IA
            </button>
          </div>
        </Panel>
      </div>

      {contexts.length > 0 && (
        <div className="form-actions">
          <button className="btn primary" onClick={() => go("vigia")}>
            Listo, ir a Vigía
          </button>
        </div>
      )}
    </div>
  );
}
