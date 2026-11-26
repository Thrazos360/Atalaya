import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Empty, Panel, Pip, Tag } from "../ui";

type CliState = {
  id: string;
  label: string;
  account: string;
  install: string | null;
  url: string;
  login: string;
  login_note: string;
  installed: boolean;
  version: string | null;
  session: "lista" | "sin-sesion" | "desconocida" | "no-instalado";
  connection: string | null;
  /** Una versión antigua instalada que ya no funciona. */
  legacy: string | null;
  /** Cómo se usa gratis, si se puede. */
  free: string | null;
  /** Puede usar un modelo de este Mac o de un servicio en vez de su cuenta. */
  own_model: boolean;
  /** El modelo propio que usa ahora («modelo · dirección»). */
  model_source: string | null;
  /** Si ese modelo propio es gratis. */
  model_free: boolean;
};

/** Un modelo que un agente puede usar en vez de su cuenta. */
type ModelSource = {
  label: string;
  base_url: string;
  model: string;
  key_account: string | null;
  place: "local" | "nube";
  free: boolean;
};

const sourceKey = (s: ModelSource) => `${s.model} · ${s.base_url}`;

type LocalServer = {
  id: string;
  label: string;
  base_url: string;
  running: boolean;
  models: string[];
  installed: boolean;
  /** La app está abierta, aunque su servidor esté apagado. */
  app_open: boolean;
  /** Modelos descargados en el disco. */
  downloaded: string[];
  /** Atalaya puede encender su servidor. */
  can_start: boolean;
  app: string | null;
  url: string;
  hint: string;
};

const SESSION: Record<CliState["session"], { text: string; pip?: "ok" | "aviso" }> = {
  lista: { text: "Sesión iniciada", pip: "ok" },
  "sin-sesion": { text: "Falta iniciar sesión", pip: "aviso" },
  desconocida: { text: "Instalado · inicia sesión si no lo has hecho" },
  "no-instalado": { text: "No instalado" },
};

/** Un agente de terminal: instalarlo, iniciar sesión y usarlo en Atalaya. */
function CliRow({ cli, sources, onChange }: { cli: CliState; sources: ModelSource[]; onChange: () => void }) {
  const [state, setState] = useState(cli);
  const [busy, setBusy] = useState<string | null>(null);
  const [waiting, setWaiting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** Lo que dijo la última comprobación de verdad. */
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => setState(cli), [cli]);
  useEffect(
    () => () => {
      if (timer.current) window.clearInterval(timer.current);
    },
    [],
  );

  const check = useCallback(async () => {
    try {
      const next = await invoke<CliState>("ai_cli_check", { id: cli.id });
      setState(next);
      return next;
    } catch (e) {
      setError(String(e));
      return null;
    }
  }, [cli.id]);

  // Después de abrir la Terminal, se vuelve a mirar cada pocos segundos hasta que cambie.
  const watch = (what: "install" | "login") => {
    if (timer.current) window.clearInterval(timer.current);
    const started = Date.now();
    setWaiting(what === "install" ? "Instalando en la Terminal…" : "Esperando que inicies sesión en la Terminal…");
    timer.current = window.setInterval(async () => {
      const next = await check();
      const done = what === "install" ? next?.installed : next?.session === "lista";
      if (done || Date.now() - started > 5 * 60_000) {
        if (timer.current) window.clearInterval(timer.current);
        timer.current = null;
        setWaiting(null);
      }
    }, 4000);
  };

  // Comprobar de verdad: una pregunta corta al agente, encerrado y en solo lectura.
  const verify = async () => {
    setBusy("verify");
    setError(null);
    setResult(null);
    try {
      const text = await invoke<string>("ai_cli_verify", { id: cli.id });
      setResult({ ok: true, text: `Funciona: ${text}` });
    } catch (e) {
      setResult({ ok: false, text: String(e) });
    } finally {
      await check();
      setBusy(null);
    }
  };

  // Qué modelo usa: su cuenta o uno de este Mac o de un servicio.
  const pickModel = async (key: string) => {
    setBusy("model");
    setError(null);
    setResult(null);
    try {
      const source = sources.find((s) => sourceKey(s) === key) ?? null;
      await invoke("ai_cli_model", { id: cli.id, source });
      await check();
      onChange();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const run = async (action: "install" | "login" | "connect") => {
    setBusy(action);
    setError(null);
    try {
      if (action === "install") {
        await invoke("ai_cli_install", { id: cli.id });
        watch("install");
      } else if (action === "login") {
        await invoke("ai_cli_login", { id: cli.id });
        if (state.session !== "desconocida") watch("login");
      } else {
        await invoke("ai_cli_connect", { id: cli.id });
        await check();
        onChange();
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const s = SESSION[state.session];
  return (
    <div className="agent-row">
      <div className="agent-info">
        <span className="cell-name">
          <Pip status={result ? (result.ok ? "ok" : "falla") : s.pip} />
          {state.label}
          {(state.model_source ? state.model_free : state.free) && <Tag tone="liquen">Gratis</Tag>}
          {state.connection && <Tag>En Atalaya</Tag>}
        </span>
        <div className="stamp">{state.account}</div>
        {state.free && !state.model_source && (
          <div className="stamp">{state.free.charAt(0).toUpperCase() + state.free.slice(1)}.</div>
        )}
        <div className="stamp">
          {busy === "verify"
            ? "Comprobando: le hago una pregunta corta…"
            : (waiting ?? (result?.ok ? SESSION.lista.text : s.text))}
          {state.version && state.installed ? ` · ${state.version}` : ""}
        </div>
        {result && <div className={`stamp${result.ok ? "" : " almenara"}`}>{result.text}</div>}
        {waiting === null && state.session === "sin-sesion" && <div className="stamp">{state.login_note}</div>}
        {state.legacy && !state.installed && (
          <div className="stamp almenara">
            Tienes {state.legacy}, que ya no funciona. Instala {state.label} para reemplazarlo.
          </div>
        )}
        {state.own_model && state.installed && (
          <label className="stamp model-pick">
            Modelo:{" "}
            <select value={state.model_source ?? ""} disabled={!!busy} onChange={(e) => pickModel(e.target.value)}>
              <option value="">Su cuenta (suscripción)</option>
              {[
                { place: "local", title: "En este Mac (gratis)" },
                { place: "nube", title: "En la nube" },
              ].map((g) => {
                const list = sources.filter((s) => s.place === g.place);
                return list.length ? (
                  <optgroup key={g.place} label={g.title}>
                    {list.map((s) => (
                      <option key={sourceKey(s)} value={sourceKey(s)}>
                        {s.label}
                        {s.free && g.place === "nube" ? " · gratis" : ""}
                      </option>
                    ))}
                  </optgroup>
                ) : null;
              })}
              {state.model_source && !sources.some((s) => sourceKey(s) === state.model_source) && (
                <option value={state.model_source}>{state.model_source}</option>
              )}
            </select>
          </label>
        )}
        {state.own_model && state.installed && !sources.length && (
          <div className="stamp">
            Para usarlo gratis, enciende Ollama o LM Studio (en «En este Mac») o guarda un servicio con modelos gratis en
            Conexiones.
          </div>
        )}
        {state.model_source?.includes(":11434") && (
          <div className="stamp">En Ollama, sube Settings → Context length a 32k o más para trabajos largos.</div>
        )}
        {error && <div className="stamp almenara">{error}</div>}
      </div>
      <div className="row-actions">
        {!state.installed &&
          (state.install ? (
            <button className="btn" disabled={!!busy} onClick={() => run("install")} title={state.install}>
              Instalar
            </button>
          ) : (
            <button className="btn" onClick={() => openUrl(state.url)}>
              Cómo instalar
            </button>
          ))}
        {state.installed && state.session !== "lista" && !state.model_source && (
          <button
            className={`btn${state.session === "sin-sesion" ? " primary" : ""}`}
            disabled={!!busy}
            onClick={() => run("login")}
            title={state.login_note}
          >
            Iniciar sesión
          </button>
        )}
        {state.installed && !state.connection && state.session !== "sin-sesion" && (
          <button className="btn primary" disabled={!!busy} onClick={() => run("connect")}>
            Usar en Atalaya
          </button>
        )}
        {state.installed && (
          <button
            className="btn"
            disabled={!!busy}
            onClick={verify}
            title="Le hace una pregunta corta para ver que responde con tu cuenta (gasta muy poco)"
          >
            {busy === "verify" ? "Comprobando…" : "Comprobar"}
          </button>
        )}
      </div>
    </div>
  );
}

/** Un servidor de modelos de este Mac. */
function LocalRow({
  server,
  added,
  onAdded,
  onReload,
}: {
  server: LocalServer;
  added: Set<string>;
  onAdded: (key: string) => void;
  onReload: () => void;
}) {
  const [model, setModel] = useState(server.models[0] ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setModel((m) => (server.models.includes(m) ? m : (server.models[0] ?? ""))), [server.models]);

  const add = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke("ai_local_add", { server: server.id, model });
      onAdded(`${server.id}:${model}`);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const open = async () => {
    try {
      await invoke("ai_local_open", { server: server.id });
    } catch (e) {
      setError(String(e));
    }
  };

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      await invoke("ai_local_start", { server: server.id });
      onReload();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const ready = server.downloaded.filter((m) => !m.endsWith("(descargando)"));
  const onDisk = server.downloaded.length
    ? ` · descargados: ${server.downloaded.join(", ")}`
    : server.installed
      ? " · sin modelos descargados"
      : "";
  const status = server.running
    ? server.models.length
      ? `${server.models.length} ${server.models.length === 1 ? "modelo" : "modelos"} · ${server.base_url}`
      : `Servidor encendido, pero sin modelos listos${onDisk}`
    : server.installed
      ? `${server.app_open ? "Abierto, pero su servidor local está apagado" : "Instalado, pero cerrado"}${onDisk}`
      : "No instalado";
  // Qué falta hacer, cuando está instalado y no hay modelos para usar.
  const showHint = server.installed && !(server.running && server.models.length > 0);

  return (
    <div className="agent-row">
      <div className="agent-info">
        <span className="cell-name">
          <Pip status={server.running ? "ok" : undefined} />
          {server.label}
        </span>
        <div className="stamp">{status}</div>
        {showHint && (
          <div className="stamp">
            {server.running && !ready.length && server.downloaded.length ? "Espera a que termine la descarga." : server.hint}
          </div>
        )}
        {error && <div className="stamp almenara">{error}</div>}
      </div>
      <div className="row-actions">
        {server.running && server.models.length > 0 && (
          <>
            <select value={model} onChange={(e) => setModel(e.target.value)} aria-label={`Modelo de ${server.label}`}>
              {server.models.map((m) => (
                <option key={m} value={m}>
                  {m}
                </option>
              ))}
            </select>
            {added.has(`${server.id}:${model}`) ? (
              <Tag tone="liquen">En Atalaya</Tag>
            ) : (
              <button className="btn primary" disabled={busy || !model} onClick={add}>
                Usar en Atalaya
              </button>
            )}
          </>
        )}
        {!server.running && server.can_start && (
          <button className="btn primary" disabled={busy} onClick={start}>
            {busy ? "Encendiendo…" : "Encender servidor"}
          </button>
        )}
        {!server.running && server.installed && server.app && (
          <button className="btn" onClick={open}>
            Abrir {server.app}
          </button>
        )}
        {!server.running && !server.installed && (
          <button className="btn" onClick={() => openUrl(server.url)}>
            Descargar
          </button>
        )}
      </div>
    </div>
  );
}

/**
 * Conectar IA sin llaves de API: los agentes de terminal inician sesión con tu cuenta
 * (como un lanzador), y los modelos locales se encuentran solos.
 */
export function Conectar({ onChange, goApi }: { onChange: () => void; goApi: () => void }) {
  const [clis, setClis] = useState<CliState[] | null>(null);
  const [sources, setSources] = useState<ModelSource[]>([]);
  const [local, setLocal] = useState<LocalServer[] | null>(null);
  const [added, setAdded] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);

  const reloadLocal = useCallback(() => {
    invoke<LocalServer[]>("ai_local")
      .then(setLocal)
      .catch((e) => setError(String(e)));
  }, []);
  const load = useCallback(() => {
    invoke<CliState[]>("ai_clis")
      .then(setClis)
      .catch((e) => setError(String(e)));
    invoke<ModelSource[]>("ai_model_sources")
      .then(setSources)
      .catch(() => setSources([]));
    reloadLocal();
  }, [reloadLocal]);
  useEffect(load, [load]);

  // Los que se pueden usar gratis primero; luego los instalados; dentro de cada grupo, el orden del catálogo.
  const isFree = (c: CliState) => Boolean(c.model_source ? c.model_free : c.free);
  const sorted = clis
    ? [...clis].sort((a, b) => Number(isFree(b)) - Number(isFree(a)) || Number(b.installed) - Number(a.installed))
    : null;

  return (
    <div className="split">
      <Panel
        title="Con tu cuenta"
        aside={
          <button className="btn" onClick={load}>
            Actualizar
          </button>
        }
        flush
      >
        <div className="panel-body">
          <p>
            Los que tienen versión gratis van primero. También sirve la suscripción que ya pagas (Claude, ChatGPT, Copilot,
            Cursor…), sin llaves de API: Atalaya abre la Terminal con el inicio de sesión de cada agente y tú entras en el
            navegador. Qwen Code puede usar un modelo de tu Mac o de la nube, gratis y sin cuenta.
          </p>
        </div>
        {error && <div className="error-note">{error}</div>}
        {sorted ? (
          <div className="agent-list">
            {sorted.map((c) => (
              <CliRow key={c.id} cli={c} sources={sources} onChange={onChange} />
            ))}
          </div>
        ) : (
          <div className="panel-body">
            <Empty>Buscando agentes en este Mac…</Empty>
          </div>
        )}
      </Panel>

      <Panel
        title="En este Mac"
        aside={
          <button className="btn" onClick={reloadLocal}>
            Actualizar
          </button>
        }
        flush
      >
        <div className="panel-body">
          <p>
            Modelos que corren en tu equipo, sin cuenta, llave ni costo por token. Atalaya habla con el servidor local de cada
            app: si está apagado, te dice cómo encenderlo.
          </p>
        </div>
        {local ? (
          <div className="agent-list">
            {local.map((s) => (
              <LocalRow
                key={s.id}
                server={s}
                added={added}
                onReload={reloadLocal}
                onAdded={(key) => {
                  setAdded(new Set([...added, key]));
                  onChange();
                }}
              />
            ))}
          </div>
        ) : (
          <div className="panel-body">
            <Empty>Buscando modelos locales…</Empty>
          </div>
        )}
        <div className="panel-body">
          <p className="footnote">
            ¿Prefieres una llave de API? Con uso gratis: OpenRouter (modelos «:free»), Groq, Mistral, SambaNova, Cloudflare,
            NVIDIA y Z.ai. De pago: DeepSeek, xAI, Kimi, Qwen y más. Todos están en{" "}
            <button className="link plain" onClick={goApi}>
              Conexiones
            </button>
            .
          </p>
        </div>
      </Panel>
    </div>
  );
}
