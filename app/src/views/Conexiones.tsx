import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { type ConnectionItem, type Connections, type Provider, api, ago } from "../api";
import { Empty, Panel, Pip, Tag } from "../ui";

type Draft = {
  id?: string;
  provider: string;
  name: string;
  config: Record<string, string>;
  secret: string;
};

const emptyDraft = (provider: Provider): Draft => ({ provider: provider.id, name: "", config: {}, secret: "" });

/** Un servicio con la API de OpenAI: elegirlo llena la dirección. */
type Preset = {
  id: string;
  label: string;
  base_url: string;
  key_url: string;
  model_hint: string;
  free: string | null;
  note: string | null;
};

/** Formulario de una conexión: nombre, los campos del proveedor y la llave (que nunca se muestra). */
function ConnectionForm({
  data,
  initial,
  onDone,
  onCancel,
}: {
  data: Connections;
  initial: Draft | null;
  onDone: (message: string) => void;
  onCancel: () => void;
}) {
  const editing = Boolean(initial?.id);
  const [kind, setKind] = useState(() => {
    const p = data.providers.find((x) => x.id === initial?.provider);
    return p?.kind ?? data.kinds[0]?.id ?? "git";
  });
  const options = data.providers.filter((p) => p.kind === kind);
  const [draft, setDraft] = useState<Draft>(() => initial ?? emptyDraft(options[0] ?? data.providers[0]));
  const provider = data.providers.find((p) => p.id === draft.provider) ?? options[0];
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [presets, setPresets] = useState<Preset[]>([]);
  const [preset, setPreset] = useState<Preset | null>(null);

  useEffect(() => {
    invoke<Preset[]>("ai_presets")
      .then(setPresets)
      .catch(() => {});
  }, []);

  const pickPreset = (id: string) => {
    const p = presets.find((x) => x.id === id) ?? null;
    setPreset(p);
    if (p)
      setDraft((d) => ({ ...d, name: d.name || p.label.replace(/ \(.*\)$/, ""), config: { ...d.config, base_url: p.base_url } }));
  };

  // Un proveedor de una sola conexión que ya existe no se ofrece para crear otra.
  const taken = (p: Provider) => !editing && p.single && data.connections.some((c) => c.provider === p.id);

  const pickKind = (next: string) => {
    setKind(next);
    const first = data.providers.find((p) => p.kind === next && !taken(p));
    if (first) setDraft(emptyDraft(first));
  };

  if (!provider) return null;
  // Una dirección que aún lleva {cuenta} (Cloudflare) no está lista.
  const missing = provider.fields.some(
    (f) => (f.required && !draft.config[f.key]?.trim()) || draft.config[f.key]?.includes("{cuenta}"),
  );
  // Una llave marcada «opcional» (como la de Gemini CLI) no impide guardar.
  const optionalSecret = provider.secret?.includes("opcional") ?? false;
  const needsSecret = Boolean(provider.secret) && !optionalSecret && !editing && !draft.secret.trim();

  const save = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const saved = await api.saveConnection({
        id: draft.id,
        provider: draft.provider,
        name: draft.name,
        config: draft.config,
        secret: draft.secret.trim() || undefined,
      });
      let message = `«${saved.name}» guardada.`;
      try {
        message += ` Prueba: ${await api.testConnection(saved.id)}`;
      } catch (err) {
        message += ` La prueba falló: ${String(err)}`;
      }
      onDone(message);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="panel form" onSubmit={save}>
      {!editing && (
        <div className="tabs">
          {data.kinds.map((k) => (
            <button type="button" key={k.id} className={kind === k.id ? "tab active" : "tab"} onClick={() => pickKind(k.id)}>
              {k.label}
            </button>
          ))}
        </div>
      )}
      <div className="form-row">
        <label>
          Proveedor
          <select
            value={draft.provider}
            disabled={editing}
            onChange={(e) => {
              const p = data.providers.find((x) => x.id === e.target.value);
              if (p) setDraft(emptyDraft(p));
            }}
          >
            {options.map((p) => (
              <option key={p.id} value={p.id} disabled={taken(p)}>
                {p.label}
                {taken(p) ? " (ya conectado)" : ""}
              </option>
            ))}
          </select>
        </label>
        <label className="grow">
          Nombre
          <input
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
            placeholder={provider.label}
            autoFocus
          />
        </label>
      </div>
      <p className="footnote">{provider.description}</p>
      {provider.id === "openai-compatible" && !editing && presets.length > 0 && (
        <label>
          Servicio
          <select value={preset?.id ?? ""} onChange={(e) => pickPreset(e.target.value)}>
            <option value="">Otro (escribe la dirección)</option>
            {presets.map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
                {p.free ? " · gratis" : ""}
              </option>
            ))}
          </select>
          {preset?.free && <span className="stamp">Gratis: {preset.free}.</span>}
          {preset?.note && <span className="stamp almenara">{preset.note}</span>}
        </label>
      )}
      {provider.fields
        .filter((f) => !f.hidden)
        .map((f) => (
          <label key={f.key}>
            {f.label}
            {f.required ? " *" : ""}
            <input
              value={draft.config[f.key] ?? ""}
              onChange={(e) => setDraft({ ...draft, config: { ...draft.config, [f.key]: e.target.value } })}
              placeholder={
                f.key === "model" && preset
                  ? preset.model_hint.includes(" ")
                    ? preset.model_hint
                    : `por ejemplo ${preset.model_hint}`
                  : f.placeholder
              }
              spellCheck={false}
            />
          </label>
        ))}
      {provider.secret && (
        <label>
          {provider.secret}
          {editing ? " (déjala vacía para conservar la que hay)" : optionalSecret ? "" : " *"}
          <input
            type="password"
            value={draft.secret}
            onChange={(e) => setDraft({ ...draft, secret: e.target.value })}
            placeholder="Se guarda en el Llavero de macOS"
            autoComplete="off"
            spellCheck={false}
          />
        </label>
      )}
      {error && <div className="error-note">{error}</div>}
      <div className="form-actions spread">
        <span className="stamp">
          {(preset?.key_url || provider.help_url) && (
            <a href={preset?.key_url || provider.help_url} target="_blank" rel="noreferrer">
              {provider.secret ? "Dónde se crea la llave" : "Más información"}
            </a>
          )}
        </span>
        <span className="card-actions">
          <button type="button" className="btn" onClick={onCancel}>
            Cancelar
          </button>
          <button className="btn primary" disabled={busy || missing || needsSecret}>
            {busy ? "Guardando y probando…" : editing ? "Guardar cambios" : "Guardar y probar"}
          </button>
        </span>
      </div>
    </form>
  );
}

/** Una fila: estado de la última prueba, quién la usa y sus acciones. */
function Row({
  item,
  provider,
  onEdit,
  onChanged,
}: {
  item: ConnectionItem;
  provider?: Provider;
  onEdit: () => void;
  onChanged: (message: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [forget, setForget] = useState(false);
  const test = item.last_test;

  const runTest = async () => {
    setBusy(true);
    try {
      onChanged(`${item.name}: ${await api.testConnection(item.id)}`);
    } catch (e) {
      onChanged(`${item.name}: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    setBusy(true);
    try {
      await api.removeConnection(item.id, forget);
      onChanged(`«${item.name}» quitada${forget ? " y su llave borrada del Llavero" : ""}.`);
    } catch (e) {
      onChanged(String(e));
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  };

  const summary = provider?.fields
    .filter((f) => !f.hidden)
    .map((f) => item.config[f.key])
    .filter(Boolean)
    .slice(0, 2)
    .join(" · ");

  return (
    <tr>
      <td>
        <div className="strong">{item.name}</div>
        <div className="stamp">
          {provider?.label ?? item.provider}
          {summary ? ` · ${summary}` : ""}
          {item.legacy ? " · llave de antes" : ""}
        </div>
      </td>
      <td>
        <span className="cell-name">
          <Pip status={test ? (test.ok ? "ok" : "falla") : undefined} />
          {test ? (test.ok ? "Probada" : "Falló") : "Sin probar"}
        </span>
        {test && (
          <div className="stamp" title={test.message}>
            {test.message.length > 80 ? `${test.message.slice(0, 80)}…` : test.message} · {ago(test.at)}
          </div>
        )}
        {provider?.secret && !provider.secret.includes("opcional") && !item.has_secret && (
          <div className="stamp almenara">Falta la llave</div>
        )}
      </td>
      <td className="stamp">{item.used_by.length ? item.used_by.join(", ") : "Ningún proyecto"}</td>
      <td className="shrink">
        {confirming ? (
          <div className="row-actions">
            {!item.legacy && provider?.secret && (
              <label className="check stamp">
                <input type="checkbox" checked={forget} onChange={(e) => setForget(e.target.checked)} /> Borrar también su llave
              </label>
            )}
            <button className="btn" onClick={() => setConfirming(false)} disabled={busy}>
              No
            </button>
            <button className="btn danger" onClick={remove} disabled={busy}>
              Quitar
            </button>
          </div>
        ) : (
          <div className="row-actions">
            <button className="btn" onClick={runTest} disabled={busy}>
              {busy ? "Probando…" : "Probar"}
            </button>
            <button className="btn" onClick={onEdit} disabled={busy}>
              Editar
            </button>
            <button className="btn danger" onClick={() => setConfirming(true)} disabled={busy}>
              Quitar
            </button>
          </div>
        )}
      </td>
    </tr>
  );
}

export function Conexiones() {
  const [data, setData] = useState<Connections | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [form, setForm] = useState<{ initial: Draft | null } | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api.connections());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  const byKind = useMemo(() => {
    const groups: Record<string, ConnectionItem[]> = {};
    for (const c of data?.connections ?? []) (groups[c.kind] ??= []).push(c);
    return groups;
  }, [data]);

  if (error) return <div className="banner">{error}</div>;
  if (!data) return <Empty>Cargando conexiones…</Empty>;

  const changed = async (message: string) => {
    setNotice(message);
    setForm(null);
    await load();
  };

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Conexiones</h1>
          <p>
            Las cuentas, bases, servidores e IA que usa Atalaya. Puede haber varias del mismo proveedor y cada proyecto elige las
            suyas. Las llaves van al Llavero de macOS y no vuelven a mostrarse.
          </p>
        </div>
        {!form && (
          <button className="btn primary" onClick={() => setForm({ initial: null })}>
            Agregar conexión
          </button>
        )}
      </header>

      {notice && <div className="banner">{notice}</div>}

      {form && <ConnectionForm data={data} initial={form.initial} onDone={changed} onCancel={() => setForm(null)} />}

      {data.connections.length === 0 && !form && (
        <Panel title="Sin conexiones">
          <Empty>
            Todavía no hay conexiones. Agrega la primera: tu repositorio (GitHub, GitLab o Gitea), dónde publicas (Vercel, Netlify
            o tu propio servidor), tus bases de datos (Supabase, Firebase, Postgres, MySQL o un archivo SQLite) y, si quieres, una
            IA.
          </Empty>
        </Panel>
      )}

      {data.kinds
        .filter((k) => byKind[k.id]?.length)
        .map((k) => (
          <Panel key={k.id} title={k.label} aside={<Tag>{byKind[k.id].length}</Tag>} flush>
            <table className="table">
              <thead>
                <tr>
                  <th>Conexión</th>
                  <th>Estado</th>
                  <th>Proyectos</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {byKind[k.id].map((item) => (
                  <Row
                    key={item.id}
                    item={item}
                    provider={data.providers.find((p) => p.id === item.provider)}
                    onChanged={changed}
                    onEdit={() =>
                      setForm({
                        initial: {
                          id: item.id,
                          provider: item.provider,
                          name: item.name,
                          config: { ...item.config },
                          secret: "",
                        },
                      })
                    }
                  />
                ))}
              </tbody>
            </table>
          </Panel>
        ))}
    </div>
  );
}
