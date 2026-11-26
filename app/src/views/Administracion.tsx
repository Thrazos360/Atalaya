import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Empty, Pip } from "../ui";

/**
 * Administración: los controles que la app ofrece por su puerta para Atalaya
 * (protocolo 1). La app describe sus secciones, elementos y controles; aquí solo
 * se dibujan y se manda un cambio a la vez. La app valida y audita cada uno.
 */

type Value = string | number | boolean | null;

type Control = {
  key: string;
  label: string;
  type: "switch" | "select" | "number" | "text" | "textarea" | "date" | "action";
  value?: Value;
  options?: { value: string; label: string }[];
  help?: string;
  group?: string;
  optional?: boolean;
  danger?: boolean;
  min?: number;
  max?: number;
  maxLength?: number;
};

type Item = { id: string; title: string; detail?: string; tone?: "ok" | "aviso" | "falla"; controls: Control[] };
type Section = { id: string; title: string; description?: string; items: Item[]; create?: { label: string; controls: Control[] } };
type Schema = { protocol: number; app: string; sections: Section[] };
type Door = { url: string | null; has_key: boolean };
type Change = { section: string; item?: string; key?: string; value?: Value; create?: Record<string, Value> };

/** Cómo se lee un valor en una confirmación. */
function shown(c: Control, v: Value) {
  if (v === null || v === "") return "vacío";
  if (typeof v === "boolean") return v ? "sí" : "no";
  return c.options?.find((o) => o.value === v)?.label ?? String(v);
}

export function AdminPanel({ slug }: { slug: string }) {
  const [door, setDoor] = useState<Door | null>(null);
  const [schema, setSchema] = useState<Schema | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [sectionId, setSectionId] = useState<string | null>(null);
  const [itemId, setItemId] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [pending, setPending] = useState<{ change: Change; text: string } | null>(null);
  const [url, setUrl] = useState("");
  const [editingUrl, setEditingUrl] = useState(false);

  const load = useCallback(async () => {
    setError(null);
    try {
      const d = await invoke<Door>("admin_door", { slug });
      setDoor(d);
      if (d.url && d.has_key) setSchema(await invoke<Schema>("admin_schema", { slug }));
    } catch (e) {
      setError(String(e));
    }
  }, [slug]);

  useEffect(() => {
    setDoor(null);
    setSchema(null);
    setSectionId(null);
    setItemId(null);
    setNotice(null);
    setPending(null);
    setEditingUrl(false);
    load();
  }, [load]);

  async function send(change: Change) {
    setBusy(true);
    setError(null);
    setNotice(null);
    setPending(null);
    try {
      setNotice(await invoke<string>("admin_apply", { slug, change }));
      setSchema(await invoke<Schema>("admin_schema", { slug }));
      return true;
    } catch (e) {
      setError(String(e));
      // Se vuelve a leer para que el control muestre lo que de verdad quedó.
      invoke<Schema>("admin_schema", { slug }).then(setSchema).catch(() => {});
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function connect(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const done = await invoke<{ note: string }>("admin_connect", { slug, url });
      setNotice(done.note);
      setEditingUrl(false);
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const section = schema?.sections.find((s) => s.id === sectionId) ?? schema?.sections[0] ?? null;
  const items = useMemo(() => {
    const q = filter.trim().toLowerCase();
    return (section?.items ?? []).filter((i) => !q || `${i.title} ${i.detail ?? ""}`.toLowerCase().includes(q));
  }, [section, filter]);
  const item = section?.items.find((i) => i.id === itemId) ?? items[0] ?? null;

  /** Un cambio delicado espera confirmación; el resto sale de inmediato. */
  function change(c: Control, value: Value) {
    if (!section || !item) return;
    const req: Change = { section: section.id, item: item.id, key: c.key, value };
    if (c.danger) {
      setPending({ change: req, text: c.type === "action" ? `${c.label}: ${item.title}` : `${item.title} · ${c.label}: ${shown(c, c.value ?? null)} → ${shown(c, value)}` });
    } else {
      send(req);
    }
  }

  const aside = door?.url ? (
    <span className="admin-door">
      <span className="stamp">{door.url.replace(/^https?:\/\//, "").replace(/\/api\/atalaya$/, "")}</span>
      <button className="link" onClick={() => setEditingUrl((v) => !v)}>
        {editingUrl ? "Cancelar" : "Cambiar"}
      </button>
    </span>
  ) : undefined;

  const connectForm = (
    <form className="inline-form admin-connect" onSubmit={connect}>
      <input
        value={url}
        onChange={(e) => setUrl(e.target.value)}
        placeholder={door?.url ?? "admin.tuapp.com"}
        spellCheck={false}
        autoFocus={editingUrl}
      />
      <button className="btn primary" disabled={busy || !url.trim()}>
        {busy ? "Conectando…" : "Conectar"}
      </button>
    </form>
  );

  return (
    <section className="panel admin">
      <header className="panel-head">
        <h2>Administración{schema ? ` · ${schema.app}` : ""}</h2>
        {aside && <div className="panel-aside">{aside}</div>}
      </header>

      {error && <p className="error-note pad">{error}</p>}
      {notice && <div className="banner ok admin-notice">{notice}</div>}
      {pending && (
        <div className="admin-confirm">
          <span>{pending.text}</span>
          <div className="actions">
            <button className="btn" onClick={() => setPending(null)}>
              Cancelar
            </button>
            <button className="btn primary" disabled={busy} onClick={() => send(pending.change)}>
              Confirmar: sale a producción
            </button>
          </div>
        </div>
      )}

      {door === null ? (
        <div className="panel-body">
          <Empty>Leyendo…</Empty>
        </div>
      ) : !door.url || !door.has_key || editingUrl ? (
        <div className="panel-body">
          <p className="admin-intro">
            Conecta la puerta de administración de esta app para manejar sus controles desde aquí. Atalaya genera una llave,
            la guarda en tu Llavero y, si la app está en Vercel, también como variable de producción. La app tiene que tener
            desplegada su ruta <span className="mono">/api/atalaya</span>.
          </p>
          {connectForm}
        </div>
      ) : !schema ? (
        <div className="panel-body">
          {error ? (
            <button className="btn" onClick={load}>
              Reintentar
            </button>
          ) : (
            <Empty>Pidiendo los controles a la app…</Empty>
          )}
        </div>
      ) : (
        <>
          {schema.sections.length > 1 && (
            <div className="tabs admin-tabs">
              {schema.sections.map((s) => (
                <button
                  key={s.id}
                  className={s.id === section?.id ? "tab active" : "tab"}
                  onClick={() => {
                    setSectionId(s.id);
                    setItemId(null);
                    setFilter("");
                    setPending(null);
                  }}
                >
                  {s.title} <span className="count">{s.items.length}</span>
                </button>
              ))}
            </div>
          )}
          {section && (
            <div className="admin-body">
              {section.description && <p className="admin-intro">{section.description}</p>}
              {section.create && <CreateForm key={section.id} spec={section.create} busy={busy} onSend={(values) => send({ section: section.id, create: values })} />}
              {section.items.length ? (
                <div className="admin-split">
                  <div className="admin-list">
                    {section.items.length > 6 && (
                      <input className="admin-filter" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder="Buscar" />
                    )}
                    {items.map((i) => (
                      <button
                        key={i.id}
                        className={i.id === item?.id ? "decision-item active" : "decision-item"}
                        onClick={() => {
                          setItemId(i.id);
                          setPending(null);
                        }}
                      >
                        <span className="decision-title">
                          <Pip status={i.tone} /> {i.title}
                        </span>
                        {i.detail && <span className="stamp">{i.detail}</span>}
                      </button>
                    ))}
                  </div>
                  {item && <ItemDetail key={`${section.id}:${item.id}`} item={item} busy={busy} onChange={change} />}
                </div>
              ) : (
                <Empty>Nada en esta sección todavía.</Empty>
              )}
            </div>
          )}
        </>
      )}
    </section>
  );
}

function ItemDetail({ item, busy, onChange }: { item: Item; busy: boolean; onChange: (c: Control, v: Value) => void }) {
  const groups: [string, Control[]][] = [];
  for (const c of item.controls) {
    const name = c.group ?? "";
    const g = groups.find(([n]) => n === name);
    if (g) g[1].push(c);
    else groups.push([name, [c]]);
  }
  return (
    <div className="admin-detail">
      <h3 className="decision-heading">{item.title}</h3>
      {item.detail && <p className="stamp">{item.detail}</p>}
      {!item.controls.length && <Empty>Sin controles disponibles.</Empty>}
      {groups.map(([name, controls]) => (
        <div key={name} className="admin-group">
          {name && <div className="field-label">{name}</div>}
          {controls.map((c) => (
            <div key={c.key} className="admin-row">
              <div className="admin-label">
                <span>{c.label}</span>
                {c.help && <span className="admin-help">{c.help}</span>}
              </div>
              <Field control={c} busy={busy} onChange={(v) => onChange(c, v)} />
            </div>
          ))}
        </div>
      ))}
    </div>
  );
}

/** Un control. Los de texto y número se mandan al salir del campo o con Enter, solo si cambiaron. */
function Field({ control: c, busy, onChange }: { control: Control; busy: boolean; onChange: (v: Value) => void }) {
  const current = c.value ?? null;
  switch (c.type) {
    case "switch":
      return (
        <button
          role="switch"
          aria-checked={Boolean(current)}
          aria-label={c.label}
          className={current ? "switch on" : "switch"}
          disabled={busy}
          onClick={() => onChange(!current)}
        />
      );
    case "select":
      return (
        <select value={current === null ? "" : String(current)} disabled={busy} onChange={(e) => onChange(e.target.value || null)}>
          {current === null && !c.options?.some((o) => o.value === "") && <option value="">—</option>}
          {c.options?.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </select>
      );
    case "action":
      return (
        <button className={c.danger ? "btn danger" : "btn"} disabled={busy} onClick={() => onChange(null)}>
          {c.label}
        </button>
      );
    default: {
      const text = current === null ? "" : String(current);
      const commit = (raw: string) => {
        const t = raw.trim();
        if (t === text) return;
        if (!t) return onChange(null);
        onChange(c.type === "number" ? Number(t) : t);
      };
      return (
        <input
          key={text}
          type={c.type === "number" ? "number" : c.type === "date" ? "date" : "text"}
          defaultValue={text}
          min={c.min}
          max={c.max}
          maxLength={c.maxLength}
          placeholder={c.optional ? "Vacío" : undefined}
          disabled={busy}
          onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          onBlur={(e) => commit(e.currentTarget.value)}
        />
      );
    }
  }
}

function CreateForm({ spec, busy, onSend }: { spec: NonNullable<Section["create"]>; busy: boolean; onSend: (v: Record<string, Value>) => Promise<boolean> }) {
  const initial = () => Object.fromEntries(spec.controls.map((c) => [c.key, c.value ?? null])) as Record<string, Value>;
  const [open, setOpen] = useState(false);
  const [values, setValues] = useState(initial);
  const set = (key: string, v: Value) => setValues((all) => ({ ...all, [key]: v }));
  const missing = spec.controls.some((c) => !c.optional && c.type !== "switch" && (values[c.key] === null || values[c.key] === ""));

  if (!open) {
    return (
      <button className="btn admin-create-open" onClick={() => setOpen(true)}>
        + {spec.label}
      </button>
    );
  }
  return (
    <form
      className="form bordered admin-create"
      onSubmit={async (e) => {
        e.preventDefault();
        if (await onSend(values)) {
          setValues(initial());
          setOpen(false);
        }
      }}
    >
      {spec.controls.map((c) => (
        <label key={c.key}>
          {c.label}
          {c.type === "textarea" ? (
            <textarea rows={3} maxLength={c.maxLength} value={String(values[c.key] ?? "")} onChange={(e) => set(c.key, e.target.value)} />
          ) : c.type === "select" ? (
            <select value={String(values[c.key] ?? "")} onChange={(e) => set(c.key, e.target.value)}>
              {c.options?.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          ) : c.type === "switch" ? (
            <input type="checkbox" checked={Boolean(values[c.key])} onChange={(e) => set(c.key, e.target.checked)} />
          ) : (
            <input
              type={c.type === "number" ? "number" : c.type === "date" ? "date" : "text"}
              maxLength={c.maxLength}
              value={String(values[c.key] ?? "")}
              onChange={(e) => set(c.key, c.type === "number" && e.target.value ? Number(e.target.value) : e.target.value)}
            />
          )}
        </label>
      ))}
      <p className="admin-help">Sale a producción apenas lo envías.</p>
      <div className="form-actions spread">
        <button type="button" className="btn" onClick={() => setOpen(false)}>
          Cancelar
        </button>
        <button className="btn primary" disabled={busy || missing}>
          {busy ? "Enviando…" : spec.label}
        </button>
      </div>
    </form>
  );
}
