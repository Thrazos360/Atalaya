import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Empty, Panel, Tag } from "../ui";
import type { Memory } from "./ChatBoard";
import type { Option } from "./Scope";

/**
 * La memoria (ver `memories.rs`): lo que las IA proponen recordar y tú apruebas, más lo que
 * escribas tú. Lo aprobado llega a todas las IA junto a las instrucciones. Y los comandos
 * guardados (`commands.rs`), que se llaman desde el chat con `/nombre`.
 */

function MemoryRow({
  item,
  projectName,
  onChanged,
}: {
  item: Memory;
  projectName: (s: string) => string;
  onChanged: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(item.text);
  const [forgetting, setForgetting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const act = async (task: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await task();
      setEditing(false);
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const proposed = item.status === "propuesta";

  return (
    <li className="skill">
      <div className="skill-head">
        <Tag>{item.slug ? projectName(item.slug) : "Todos"}</Tag>
        {proposed && <Tag tone="almenara">Propuesta</Tag>}
        <span className="stamp">
          {item.source === "usuario" ? "tuya" : `de ${item.source}`} · {item.created_at.slice(0, 10)}
        </span>
        <div className="skill-actions">
          {proposed && !editing && (
            <button
              type="button"
              className="btn primary"
              disabled={busy}
              onClick={() => act(() => invoke("memories_decide", { id: item.id, approve: true }))}
            >
              Aprobar
            </button>
          )}
          {!editing && (
            <button type="button" className="btn" disabled={busy} onClick={() => setEditing(true)}>
              {proposed ? "Ajustar" : "Editar"}
            </button>
          )}
          {proposed ? (
            <button
              type="button"
              className="btn"
              disabled={busy}
              onClick={() => act(() => invoke("memories_decide", { id: item.id, approve: false }))}
            >
              Descartar
            </button>
          ) : forgetting ? (
            <>
              <button
                type="button"
                className="btn danger"
                disabled={busy}
                onClick={() => act(() => invoke("memories_decide", { id: item.id, approve: false }))}
              >
                Sí, olvidar
              </button>
              <button type="button" className="btn" onClick={() => setForgetting(false)}>
                No
              </button>
            </>
          ) : (
            <button
              type="button"
              className="btn"
              onClick={() => setForgetting(true)}
              title="Queda descartada; las IA dejan de recibirla"
            >
              Olvidar
            </button>
          )}
        </div>
      </div>
      {editing ? (
        <div className="form">
          <textarea rows={2} value={text} onChange={(e) => setText(e.target.value)} />
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => (setEditing(false), setText(item.text))}>
              Cancelar
            </button>
            <button
              type="button"
              className="btn primary"
              disabled={busy || !text.trim()}
              onClick={() =>
                act(() =>
                  proposed
                    ? invoke("memories_decide", { id: item.id, approve: true, text })
                    : invoke("memories_edit", { id: item.id, text }),
                )
              }
            >
              {proposed ? "Guardar y aprobar" : "Guardar"}
            </button>
          </div>
        </div>
      ) : (
        <p className="skill-description">{item.text}</p>
      )}
      {error && <p className="tablero-error">{error}</p>}
    </li>
  );
}

export function MemoryPanel({ projects }: { projects: Option[] }) {
  const [items, setItems] = useState<Memory[] | null>(null);
  const [slug, setSlug] = useState("");
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    invoke<Memory[]>("memories_list")
      .then(setItems)
      .catch((e) => setError(String(e)));
  }, []);
  useEffect(load, [load]);
  const projectName = (s: string) => projects.find((p) => p.id === s)?.name ?? s;

  const add = async (e: React.FormEvent) => {
    e.preventDefault();
    try {
      await invoke("memories_add", { slug: slug || null, text });
      setText("");
      setError(null);
      load();
    } catch (err) {
      setError(String(err));
    }
  };
  const proposed = items?.filter((m) => m.status === "propuesta") ?? [];
  const approved = items?.filter((m) => m.status === "aprobada") ?? [];

  return (
    <Panel title={`Memoria · ${approved.length}${proposed.length ? ` · ${proposed.length} por aprobar` : ""}`}>
      <p className="stamp">
        Lo que las IA deben recordar de cada proyecto y de cómo trabajas. Claude Code propone desde el chat; tú apruebas. Lo
        aprobado llega a todas las IA junto a las instrucciones.
      </p>
      {items && items.length === 0 && <Empty>Todavía no hay nada que recordar.</Empty>}
      {items && items.length > 0 && (
        <ul className="skill-list">
          {[...proposed, ...approved].map((m) => (
            <MemoryRow key={m.id} item={m} projectName={projectName} onChanged={load} />
          ))}
        </ul>
      )}
      <form className="form-row memory-add" onSubmit={add}>
        <select value={slug} onChange={(e) => setSlug(e.target.value)} aria-label="Para qué proyecto">
          <option value="">Para todos</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="Algo que recordar: «este repo usa pnpm»"
          aria-label="Memoria nueva"
        />
        <button className="btn" disabled={!text.trim()}>
          Agregar
        </button>
      </form>
      {error && <p className="tablero-error">{error}</p>}
    </Panel>
  );
}

type SavedCommand = { name: string; description: string; template: string };

/** Los de ejemplo de `commands::EXAMPLES`. */
const EXAMPLES = ["revisar-seguridad", "subir-dependencias", "explicar", "resumen-semana"];

export function CommandsPanel() {
  const [items, setItems] = useState<SavedCommand[] | null>(null);
  const [draft, setDraft] = useState<SavedCommand>({ name: "", description: "", template: "" });
  const [editing, setEditing] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(() => {
    invoke<SavedCommand[]>("commands_list")
      .then(setItems)
      .catch((e) => setError(String(e)));
  }, []);
  useEffect(load, [load]);
  const missing = items ? EXAMPLES.filter((n) => !items.some((c) => c.name === n)) : [];
  const run = async (task: () => Promise<unknown>) => {
    try {
      await task();
      setError(null);
      load();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <Panel title={`Comandos · ${items?.length ?? 0}`}>
      <p className="stamp">
        Recetas que llamas desde el chat escribiendo <code>/nombre</code> y lo que quieras después; sirven con cualquier IA. En la
        receta, <code>{"{texto}"}</code> es lo que escribas después del nombre.
      </p>
      {items && items.length > 0 && (
        <ul className="skill-list">
          {items.map((c) => (
            <li key={c.name} className="skill">
              <div className="skill-head">
                <strong>/{c.name}</strong>
                <span className="stamp">{c.description}</span>
                <div className="skill-actions">
                  <button type="button" className="btn" onClick={() => (setDraft(c), setEditing(true))}>
                    Editar
                  </button>
                  {removing === c.name ? (
                    <>
                      <button
                        type="button"
                        className="btn danger"
                        onClick={() => run(() => invoke("commands_remove", { name: c.name }))}
                      >
                        Sí, quitar
                      </button>
                      <button type="button" className="btn" onClick={() => setRemoving(null)}>
                        No
                      </button>
                    </>
                  ) : (
                    <button type="button" className="btn" onClick={() => setRemoving(c.name)}>
                      Quitar
                    </button>
                  )}
                </div>
              </div>
              <p className="skill-description clamp">{c.template}</p>
            </li>
          ))}
        </ul>
      )}
      {missing.length > 0 && (
        <p>
          <button type="button" className="btn" onClick={() => run(() => invoke("commands_examples"))}>
            Agregar los de ejemplo
          </button>{" "}
          <span className="stamp">{missing.map((n) => `/${n}`).join(", ")}</span>
        </p>
      )}
      <form
        className="form"
        onSubmit={(e) => {
          e.preventDefault();
          run(async () => {
            await invoke("commands_save", { ...draft });
            setDraft({ name: "", description: "", template: "" });
            setEditing(false);
          });
        }}
      >
        <div className="form-row">
          <label>
            Nombre
            <input
              value={draft.name}
              disabled={editing}
              onChange={(e) => setDraft({ ...draft, name: e.target.value })}
              placeholder="revisar-login"
            />
          </label>
          <label>
            Qué hace
            <input
              value={draft.description}
              onChange={(e) => setDraft({ ...draft, description: e.target.value })}
              placeholder="En una línea"
            />
          </label>
        </div>
        <label>
          Lo que le pide a la IA
          <textarea
            rows={3}
            value={draft.template}
            onChange={(e) => setDraft({ ...draft, template: e.target.value })}
            placeholder="Revisa el flujo de ingreso de {texto}: errores, mensajes y qué pasa sin conexión."
          />
        </label>
        <div className="form-actions">
          {editing && (
            <button
              type="button"
              className="btn"
              onClick={() => (setDraft({ name: "", description: "", template: "" }), setEditing(false))}
            >
              Cancelar
            </button>
          )}
          <button className="btn primary" disabled={!draft.name.trim() || !draft.template.trim()}>
            {editing ? "Guardar cambios" : "Guardar comando"}
          </button>
        </div>
      </form>
      {error && <p className="tablero-error">{error}</p>}
    </Panel>
  );
}
