import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Empty, Panel, Tag } from "../ui";
import { Scope, type Option } from "./Scope";

/**
 * Servidores MCP externos (ver `mcp_servers.rs`): se agregan una vez y llegan a Claude Code y
 * a los agentes de terminal. Se agregan apagados; se activan después de probarlos. La llave va
 * directo al Llavero y nunca vuelve aquí.
 */

type Server = {
  name: string;
  description: string;
  kind: "programa" | "web";
  command: string | null;
  args: string[];
  url: string | null;
  env: Record<string, string>;
  secret_var: string | null;
  enabled: boolean;
  projects: string[] | null;
  agents: string[] | null;
  tools: string[];
  tested_at: string | null;
  has_secret: boolean;
};
type Preset = {
  id: string;
  label: string;
  description: string;
  kind: "programa" | "web";
  command: string | null;
  args: string[];
  url: string | null;
  secret_var: string | null;
  secret_help: string;
};
type Data = { servers: Server[]; presets: Preset[] };

type Draft = {
  name: string;
  description: string;
  kind: "programa" | "web";
  command: string;
  url: string;
  env: string;
  secretVar: string;
  secret: string;
  help: string;
  editing: boolean;
  hasSecret: boolean;
};

const EMPTY: Draft = {
  name: "",
  description: "",
  kind: "programa",
  command: "",
  url: "",
  env: "",
  secretVar: "",
  secret: "",
  help: "",
  editing: false,
  hasSecret: false,
};

function target(s: Pick<Server, "kind" | "command" | "args" | "url">): string {
  return s.kind === "web" ? (s.url ?? "") : [s.command, ...s.args].filter(Boolean).join(" ");
}

function fromPreset(p: Preset): Draft {
  return {
    ...EMPTY,
    name: p.id,
    description: p.description,
    kind: p.kind,
    command: target({ ...p, kind: "programa" }),
    url: p.url ?? "",
    secretVar: p.secret_var ?? "",
    help: p.secret_help,
  };
}

function fromServer(s: Server): Draft {
  return {
    ...EMPTY,
    name: s.name,
    description: s.description,
    kind: s.kind,
    command: s.kind === "programa" ? target(s) : "",
    url: s.url ?? "",
    env: Object.entries(s.env)
      .map(([k, v]) => `${k}=${v}`)
      .join("\n"),
    secretVar: s.secret_var ?? "",
    editing: true,
    hasSecret: s.has_secret,
  };
}

/** Un servidor: probar, activar, dónde rige, editar y quitar. */
function ServerRow({
  server,
  projects,
  ais,
  onChanged,
  onEdit,
}: {
  server: Server;
  projects: Option[];
  ais: Option[];
  onChanged: () => void;
  onEdit: () => void;
}) {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [removing, setRemoving] = useState(false);

  const act = async (what: string, task: () => Promise<unknown>) => {
    setBusy(what);
    setError(null);
    try {
      await task();
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };
  const configure = (patch: Partial<Pick<Server, "enabled" | "projects" | "agents">>) =>
    act("config", () =>
      invoke("mcps_configure", {
        name: server.name,
        enabled: patch.enabled ?? server.enabled,
        projects: patch.projects !== undefined ? patch.projects : server.projects,
        agents: patch.agents !== undefined ? patch.agents : server.agents,
      }),
    );
  const tested = server.tools.length > 0;

  return (
    <li className="skill">
      <div className="skill-head">
        <strong>{server.name}</strong>
        {server.enabled ? <Tag tone="liquen">Activo</Tag> : <Tag>Apagado</Tag>}
        <Tag>{server.kind === "web" ? "Web" : "Programa"}</Tag>
        <div className="skill-actions">
          <button
            type="button"
            className="btn"
            disabled={!!busy}
            onClick={() => act("test", () => invoke("mcps_test", { name: server.name }))}
          >
            {busy === "test" ? "Probando…" : "Probar"}
          </button>
          {server.enabled ? (
            <button type="button" className="btn" disabled={!!busy} onClick={() => configure({ enabled: false })}>
              Apagar
            </button>
          ) : (
            <button
              type="button"
              className="btn primary"
              disabled={!!busy || !tested}
              title={tested ? undefined : "Pruébalo primero"}
              onClick={() => configure({ enabled: true })}
            >
              Activar
            </button>
          )}
          <button type="button" className="btn" disabled={!!busy} onClick={onEdit}>
            Editar
          </button>
          {removing ? (
            <>
              <button
                type="button"
                className="btn danger"
                disabled={!!busy}
                onClick={() => act("remove", () => invoke("mcps_remove", { name: server.name }))}
              >
                Sí, quitar
              </button>
              <button type="button" className="btn" onClick={() => setRemoving(false)}>
                No
              </button>
            </>
          ) : (
            <button type="button" className="btn" onClick={() => setRemoving(true)} title="Su llave queda en el Llavero">
              Quitar
            </button>
          )}
        </div>
      </div>
      {server.description && <p className="skill-description">{server.description}</p>}
      <p className="stamp">
        <code>{target(server)}</code>
        {server.has_secret ? " · con llave" : server.secret_var || server.kind === "web" ? " · sin llave" : ""}
      </p>
      {tested ? (
        <p className="stamp mcp-tools" title={server.tools.join(", ")}>
          {server.tools.length} herramientas: {server.tools.slice(0, 8).join(", ")}
          {server.tools.length > 8 ? "…" : ""}
        </p>
      ) : (
        <p className="stamp">Sin probar: pruébalo para ver sus herramientas y poder activarlo.</p>
      )}
      {server.enabled && (
        <div className="skill-scopes">
          <Scope
            label="Proyectos"
            all="En todos"
            value={server.projects}
            options={projects}
            disabled={!!busy}
            onChange={(projects) => configure({ projects })}
          />
          <Scope
            label="IA"
            all="Para todas"
            value={server.agents}
            options={ais}
            disabled={!!busy}
            onChange={(agents) => configure({ agents })}
          />
        </div>
      )}
      {error && <p className="tablero-error">{error}</p>}
    </li>
  );
}

/** Agregar o cambiar un servidor. */
function ServerForm({
  presets,
  draft,
  setDraft,
  onSaved,
}: {
  presets: Preset[];
  draft: Draft;
  setDraft: (d: Draft) => void;
  onSaved: (name: string) => void;
}) {
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = (patch: Partial<Draft>) => (setDraft({ ...draft, ...patch }), setError(null));

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    const words = draft.command.trim().split(/\s+/).filter(Boolean);
    const env: Record<string, string> = {};
    for (const line of draft.env
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean)) {
      const at = line.indexOf("=");
      if (at < 1) return setError(`«${line}» no es CLAVE=valor`);
      env[line.slice(0, at).trim()] = line.slice(at + 1).trim();
    }
    setSaving(true);
    try {
      const saved = await invoke<Server>("mcps_save", {
        server: {
          name: draft.name,
          description: draft.description,
          kind: draft.kind,
          command: draft.kind === "programa" ? (words[0] ?? null) : null,
          args: draft.kind === "programa" ? words.slice(1) : [],
          url: draft.kind === "web" ? draft.url.trim() : null,
          env: draft.kind === "programa" ? env : {},
          secret_var: draft.kind === "programa" ? draft.secretVar.trim() || null : null,
        },
        secret: draft.secret.trim() || null,
      });
      onSaved(saved.name);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };
  const wantsKey = draft.kind === "web" || !!draft.secretVar.trim();

  return (
    <form className="form" onSubmit={save}>
      {!draft.editing && (
        <div className="mcp-presets">
          <span className="stamp">Conocidos:</span>
          {presets.map((p) => (
            <button key={p.id} type="button" className="btn" onClick={() => setDraft(fromPreset(p))} title={p.description}>
              {p.label}
            </button>
          ))}
        </div>
      )}
      <div className="form-row">
        <label>
          Nombre
          <input
            value={draft.name}
            disabled={draft.editing}
            onChange={(e) => set({ name: e.target.value })}
            placeholder="supabase"
          />
        </label>
        <fieldset className="mcp-kind">
          <legend>Tipo</legend>
          <label className="check">
            <input type="radio" checked={draft.kind === "programa"} onChange={() => set({ kind: "programa" })} /> Programa
          </label>
          <label className="check">
            <input type="radio" checked={draft.kind === "web"} onChange={() => set({ kind: "web" })} /> Dirección web
          </label>
        </fieldset>
      </div>
      {draft.kind === "programa" ? (
        <>
          <label>
            Comando
            <input
              value={draft.command}
              onChange={(e) => set({ command: e.target.value })}
              placeholder="npx -y @upstash/context7-mcp"
            />
          </label>
          <label>
            Variables (sin secretos), una por línea
            <textarea rows={2} value={draft.env} onChange={(e) => set({ env: e.target.value })} placeholder="MODO=lectura" />
          </label>
          <label>
            Variable donde lee su llave (si pide una)
            <input
              value={draft.secretVar}
              onChange={(e) => set({ secretVar: e.target.value })}
              placeholder="SUPABASE_ACCESS_TOKEN"
            />
          </label>
        </>
      ) : (
        <label>
          Dirección
          <input value={draft.url} onChange={(e) => set({ url: e.target.value })} placeholder="https://…/mcp" />
        </label>
      )}
      {wantsKey && (
        <label>
          Llave {draft.kind === "web" ? "(va como Authorization: Bearer)" : ""}
          <input
            type="password"
            autoComplete="off"
            value={draft.secret}
            onChange={(e) => set({ secret: e.target.value })}
            placeholder={draft.hasSecret ? "Queda la que guardaste" : draft.help || "Se guarda en el Llavero"}
          />
        </label>
      )}
      <label>
        Qué hace
        <input value={draft.description} onChange={(e) => set({ description: e.target.value })} placeholder="En una línea" />
      </label>
      <div className="form-actions spread">
        <span className="stamp">
          {error ? <span className="tablero-error">{error}</span> : "Queda apagado: pruébalo y actívalo."}
        </span>
        <span>
          {draft.editing && (
            <button type="button" className="btn" onClick={() => setDraft(EMPTY)}>
              Cancelar
            </button>
          )}{" "}
          <button className="btn primary" disabled={saving || !draft.name.trim()}>
            {saving ? "Guardando…" : draft.editing ? "Guardar cambios" : "Agregar"}
          </button>
        </span>
      </div>
    </form>
  );
}

export function McpServers({ projects, ais }: { projects: Option[]; ais: Option[] }) {
  const [data, setData] = useState<Data | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [draft, setDraft] = useState<Draft>(EMPTY);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await invoke<Data>("mcps_list"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  if (!data) return error ? <div className="banner">{error}</div> : null;
  return (
    <>
      <Panel title={`Servidores MCP · ${data.servers.length}`}>
        <p className="stamp">
          Además del MCP de Atalaya. Llegan a Claude Code en el chat y a los agentes de terminal (a Gemini CLI, en las consultas).
          Los programas corren en el mismo aislamiento que los agentes.
        </p>
        {data.servers.length === 0 ? (
          <Empty>Todavía no hay servidores. Agrega uno abajo: Supabase, GitHub, Context7, Playwright o el tuyo.</Empty>
        ) : (
          <ul className="skill-list">
            {data.servers.map((s) => (
              <ServerRow
                key={s.name}
                server={s}
                projects={projects}
                ais={ais}
                onChanged={load}
                onEdit={() => (setDraft(fromServer(s)), setNote(null))}
              />
            ))}
          </ul>
        )}
      </Panel>
      <Panel title={draft.editing ? `Cambiar ${draft.name}` : "Agregar un servidor MCP"}>
        <ServerForm
          presets={data.presets}
          draft={draft}
          setDraft={setDraft}
          onSaved={(name) => {
            setNote(`Guardado: ${name}. Pruébalo y actívalo arriba.`);
            setDraft(EMPTY);
            load();
          }}
        />
        {note && <p className="stamp">{note}</p>}
      </Panel>
    </>
  );
}
