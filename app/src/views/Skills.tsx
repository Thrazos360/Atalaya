import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { Console } from "../App";
import { Empty, Panel, Tag } from "../ui";
import { McpServers } from "./McpServers";
import { CommandsPanel, MemoryPanel } from "./Memory";
import { Scope, type Option } from "./Scope";

/**
 * Skills e instrucciones comunes: se instalan una vez en Atalaya y llegan a cada IA cuando
 * trabaja (ver `skills.rs`). Una skill se instala apagada y se activa después de revisarla.
 */

type Skill = {
  name: string;
  description: string;
  source: string;
  enabled: boolean;
  projects: string[] | null;
  agents: string[] | null;
  installed_at: string;
  path: string;
};
type Found = { name: string; description: string; path: string; origin: string };
type Data = { skills: Skill[]; found: Found[]; instructions: { general: string; projects: Record<string, string> } };
type Review = { skill: Skill; text: string; files: { path: string; size: number; script: boolean }[] };
type Roles = { roles: { options: { id: string; name: string }[] }[] };

function size(bytes: number): string {
  return bytes < 1024 ? `${bytes} B` : `${Math.round(bytes / 1024)} KB`;
}

/** Una skill instalada: revisar, activar, dónde rige y quitar. */
function SkillRow({
  skill,
  projects,
  ais,
  onChanged,
}: {
  skill: Skill;
  projects: Option[];
  ais: Option[];
  onChanged: () => void;
}) {
  const [review, setReview] = useState<Review | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);

  const act = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await task();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const configure = (patch: Partial<Pick<Skill, "enabled" | "projects" | "agents">>) =>
    act(() =>
      invoke("skills_configure", {
        name: skill.name,
        enabled: patch.enabled ?? skill.enabled,
        projects: patch.projects !== undefined ? patch.projects : skill.projects,
        agents: patch.agents !== undefined ? patch.agents : skill.agents,
      }),
    );
  const toggleReview = async () => {
    if (open) return setOpen(false);
    setOpen(true);
    if (!review) {
      try {
        setReview(await invoke<Review>("skills_review", { name: skill.name }));
      } catch (e) {
        setError(String(e));
      }
    }
  };
  const scripts = review?.files.filter((f) => f.script) ?? [];

  return (
    <li className="skill">
      <div className="skill-head">
        <strong>{skill.name}</strong>
        {skill.enabled ? <Tag tone="liquen">Activa</Tag> : <Tag>Apagada</Tag>}
        <div className="skill-actions">
          <button type="button" className="btn" onClick={toggleReview} aria-expanded={open}>
            {open ? "Cerrar" : skill.enabled ? "Revisar" : "Revisar y activar"}
          </button>
          {skill.enabled && (
            <button type="button" className="btn" disabled={busy} onClick={() => configure({ enabled: false })}>
              Apagar
            </button>
          )}
          {removing ? (
            <>
              <button
                type="button"
                className="btn danger"
                disabled={busy}
                onClick={() => act(() => invoke("skills_remove", { name: skill.name }))}
              >
                Sí, a la Papelera
              </button>
              <button type="button" className="btn" onClick={() => setRemoving(false)}>
                No
              </button>
            </>
          ) : (
            <button type="button" className="btn" onClick={() => setRemoving(true)} title="Su carpeta va a la Papelera">
              Quitar
            </button>
          )}
        </div>
      </div>
      {skill.description && <p className="skill-description">{skill.description}</p>}
      <p className="stamp">De {skill.source}</p>

      {open && (
        <div className="skill-review">
          {!review ? (
            <Empty>Leyendo la skill…</Empty>
          ) : (
            <>
              <ul className="skill-files">
                {review.files.map((f) => (
                  <li key={f.path} className={f.script ? "script" : undefined}>
                    <code>{f.path}</code> <span className="stamp">{size(f.size)}</span>
                    {f.script && <Tag tone="almenara">Programa</Tag>}
                  </li>
                ))}
              </ul>
              {scripts.length > 0 && (
                <p className="skill-warning">
                  Trae {scripts.length === 1 ? "un programa" : `${scripts.length} programas`} que una IA podría ejecutar. Los
                  agentes de terminal lo hacen dentro del aislamiento de macOS; Claude Code, con sus propios permisos. Actívala
                  solo si confías en quien la escribió.
                </p>
              )}
              <pre className="skill-text">{review.text}</pre>
              {!skill.enabled && (
                <div className="form-actions">
                  <button type="button" className="btn primary" disabled={busy} onClick={() => configure({ enabled: true })}>
                    Activar
                  </button>
                </div>
              )}
            </>
          )}
        </div>
      )}

      {skill.enabled && (
        <div className="skill-scopes">
          <Scope
            label="Proyectos"
            all="En todos"
            value={skill.projects}
            options={projects}
            disabled={busy}
            onChange={(projects) => configure({ projects })}
          />
          <Scope
            label="IA"
            all="Para todas"
            value={skill.agents}
            options={ais}
            disabled={busy}
            onChange={(agents) => configure({ agents })}
          />
        </div>
      )}
      {error && <p className="tablero-error">{error}</p>}
    </li>
  );
}

/** Las instrucciones para todas las IA y las de cada proyecto. */
function Instructions({ data, projects, onSaved }: { data: Data; projects: Option[]; onSaved: () => void }) {
  const [slug, setSlug] = useState("");
  const saved = slug ? (data.instructions.projects[slug] ?? "") : data.instructions.general;
  const [text, setText] = useState(saved);
  const [state, setState] = useState<string | null>(null);
  useEffect(() => setText(saved), [saved, slug]);

  const save = async () => {
    try {
      await invoke("instructions_set", { slug: slug || null, text });
      setState("Guardadas. Las reciben desde el próximo encargo.");
      onSaved();
    } catch (e) {
      setState(String(e));
    }
  };

  return (
    <Panel title="Instrucciones">
      <div className="form">
        <label>
          Para
          <select value={slug} onChange={(e) => (setSlug(e.target.value), setState(null))}>
            <option value="">Todas las IA, en todos los proyectos</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                El proyecto {p.name}
                {data.instructions.projects[p.id] ? " ·" : ""}
              </option>
            ))}
          </select>
        </label>
        <label>
          Texto
          <textarea
            rows={5}
            value={text}
            onChange={(e) => (setText(e.target.value), setState(null))}
            placeholder={
              slug
                ? "Lo propio de este proyecto: «usamos pnpm», «las migraciones las aprueba alguien», «el diseño sigue tokens.css»…"
                : "Lo que vale siempre: «responde en español», «no subas nada sin pruebas», «prefiere cambios pequeños»…"
            }
          />
        </label>
        <div className="form-actions spread">
          <span className="stamp">{state ?? "Llegan a Claude Code, a los agentes y a los modelos por API en cada encargo."}</span>
          <button type="button" className="btn primary" disabled={text.trim() === saved.trim()} onClick={save}>
            Guardar
          </button>
        </div>
      </div>
    </Panel>
  );
}

export function Skills({ contexts }: Console) {
  const [data, setData] = useState<Data | null>(null);
  const [ais, setAis] = useState<Option[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  const [installing, setInstalling] = useState(false);
  const [showFound, setShowFound] = useState(false);
  const [filter, setFilter] = useState("");

  const load = useCallback(async () => {
    try {
      setData(await invoke<Data>("skills_list"));
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

  const needle = filter.trim().toLowerCase();
  const found = (data?.found ?? []).filter((f) => !needle || `${f.name} ${f.description}`.toLowerCase().includes(needle));
  const projects: Option[] = contexts.map((c) => ({ id: c.project.slug, name: c.project.name }));

  const install = async (task: () => Promise<Skill[]>) => {
    setInstalling(true);
    setNote(null);
    setError(null);
    try {
      const added = await task();
      setNote(
        `Instalada${added.length > 1 ? "s" : ""}: ${added.map((s) => s.name).join(", ")}. Quedan apagadas: revísalas y actívalas.`,
      );
      setUrl("");
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setInstalling(false);
    }
  };
  const fromFolder = async () => {
    const dir = await open({ directory: true, title: "Carpeta de la skill (con su SKILL.md) o de varias skills" });
    if (typeof dir === "string") install(() => invoke<Skill[]>("skills_install_folder", { path: dir }));
  };

  if (!data) return error ? <div className="banner">{error}</div> : <Empty>Cargando skills…</Empty>;

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Skills, memoria y MCP</h1>
          <p>
            Lo que instalas aquí llega a cada IA cuando trabaja, sin tocar la configuración de cada agente. Claude Code recibe las
            skills como propias; los agentes de terminal leen su SKILL.md; los modelos por API reciben su texto. Los servidores
            MCP llegan a Claude Code y a los agentes de terminal.
          </p>
        </div>
      </header>

      <Instructions data={data} projects={projects} onSaved={load} />

      <MemoryPanel projects={projects} />

      <Panel title={`Instaladas · ${data.skills.length}`}>
        {data.skills.length === 0 ? (
          <Empty>
            Todavía no hay skills. Una skill es una carpeta con un SKILL.md: un nombre, una descripción y las instrucciones para
            una tarea.
          </Empty>
        ) : (
          <ul className="skill-list">
            {data.skills.map((s) => (
              <SkillRow key={s.name} skill={s} projects={projects} ais={ais} onChanged={load} />
            ))}
          </ul>
        )}
      </Panel>

      <McpServers projects={projects} ais={ais} />

      <CommandsPanel />

      <Panel title="Instalar una skill">
        <div className="form">
          <div className="form-row">
            <button type="button" className="btn" onClick={fromFolder} disabled={installing}>
              Desde una carpeta…
            </button>
            <form
              className="skill-git"
              onSubmit={(e) => {
                e.preventDefault();
                if (url.trim()) install(() => invoke<Skill[]>("skills_install_git", { url }));
              }}
            >
              <input
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="O la URL de un repo de git: https://github.com/…"
                aria-label="URL del repo de la skill"
              />
              <button className="btn" disabled={installing || !url.trim()}>
                {installing ? "Instalando…" : "Instalar"}
              </button>
            </form>
          </div>
          {note && <p className="stamp">{note}</p>}
          {error && <p className="tablero-error">{error}</p>}
          {data.found.length > 0 && (
            <div className="skill-found">
              <button type="button" className="btn" aria-expanded={showFound} onClick={() => setShowFound(!showFound)}>
                {showFound ? "Ocultar" : "Importar de tus agentes"} · {data.found.length}
              </button>
              {showFound && (
                <>
                  <p className="stamp">
                    Ya las tienes en tus agentes, fuera de Atalaya. Al importarlas se copian; las originales quedan igual.
                  </p>
                  <input
                    value={filter}
                    onChange={(e) => setFilter(e.target.value)}
                    placeholder="Buscar por nombre o descripción"
                    aria-label="Buscar entre tus skills"
                  />
                  <ul>
                    {found.map((f) => (
                      <li key={f.path}>
                        <div>
                          <strong>{f.name}</strong> <span className="stamp">· {f.origin}</span>
                          {f.description && <p className="skill-description clamp">{f.description}</p>}
                        </div>
                        <button
                          type="button"
                          className="btn"
                          disabled={installing}
                          onClick={() =>
                            install(() =>
                              invoke<Skill[]>("skills_install_folder", { path: f.path, source: `tus skills de ${f.origin}` }),
                            )
                          }
                        >
                          Importar
                        </button>
                      </li>
                    ))}
                    {!found.length && <li className="stamp">Ninguna coincide.</li>}
                  </ul>
                </>
              )}
            </div>
          )}
        </div>
      </Panel>
    </div>
  );
}
