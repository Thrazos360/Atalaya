import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ago } from "../api";
import { Empty, Panel, Tag } from "../ui";

type Model = { model: string; usd: number; requests: number; input_tokens: number; output_tokens: number };

type ClaudeStatus = {
  connected: boolean;
  budget_usd: number | null;
  error: string | null;
  summary: {
    month: string;
    month_usd: number;
    requests: number;
    input_tokens: number;
    output_tokens: number;
    models: Model[];
    days: { day: string; usd: number }[];
    fetched_at: string;
  } | null;
};

const usd = (n: number) => `US$ ${n.toLocaleString("es", { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;

/** 1.284.500 → "1,3 M": los tokens se leen mejor resumidos. */
function compact(n: number) {
  if (n >= 1e6) return `${(n / 1e6).toLocaleString("es", { maximumFractionDigits: 1 })} M`;
  if (n >= 1e3) return `${(n / 1e3).toLocaleString("es", { maximumFractionDigits: 1 })} k`;
  return String(n);
}

type AiRow = {
  connection_id: string;
  connection: string;
  provider: string;
  model: string;
  calls: number;
  errors: number;
  input_tokens: number;
  output_tokens: number;
  cost_usd: number | null;
  unpriced_calls: number;
  billing: Billing;
};
type Billing = "gratis" | "cuenta" | "api";
type AiShare = {
  id: string;
  name: string;
  provider: string;
  billing: Billing;
  calls: number;
  errors: number;
  calls_pct: number;
  tokens: number;
  tokens_pct: number;
  untracked_calls: number;
  tasks: { task: string; calls: number }[];
  last_at: string | null;
  last_error: { text: string | null; at: string } | null;
  /** Sin cuota: Atalaya la salta hasta esa hora. */
  paused: { until: string; reason: string } | null;
};
type AiConnection = {
  id: string;
  name: string;
  kind: string;
  provider: string;
  model: string | null;
  local: boolean;
  budget_usd: number | null;
  spent_usd: number;
};
type AiUsage = {
  month_usd: number;
  by_ai: { calls: number; tokens: number; free_pct: number; ais: AiShare[] };
  by_model: AiRow[];
  by_task: { task: string; calls: number; tokens: number; cost_usd: number | null }[];
  connections: AiConnection[];
  task_cap_usd: number | null;
  prices: {
    provider: string;
    model: string;
    input_per_mtok: number;
    output_per_mtok: number;
    source: string;
    updated_at: string;
  }[];
};

const TASK_LABEL: Record<string, string> = {
  prueba: "Pruebas",
  agente: "Agentes",
  ronda: "Ronda diaria",
  revision: "Revisión",
  chat: "Chat",
  radar: "Radar",
};

const BILLING: Record<Billing, { label: string; tone?: "liquen" | "almenara"; note: string }> = {
  gratis: { label: "Gratis", tone: "liquen", note: "no se paga" },
  cuenta: { label: "Tu cuenta", note: "lo cubre tu suscripción o plan" },
  api: { label: "Por uso", tone: "almenara", note: "se cobra por token" },
};

const clip = (text: string, max: number) => (text.length > max ? `${text.slice(0, max)}…` : text);

const percent = (n: number) => `${n.toLocaleString("es", { maximumFractionDigits: 1 })} %`;

/** Quién hizo el trabajo del mes: la parte de cada IA, también la gratis, para ver si de verdad se reparten. */
function Reparto({ share, onChanged }: { share: AiUsage["by_ai"]; onChanged: () => void }) {
  const retry = async (id: string) => {
    await invoke("ai_quota_clear", { connectionId: id });
    onChanged();
  };
  if (!share.calls)
    return (
      <Panel title="Quién hizo el trabajo" aside="este mes">
        <Empty>Ninguna IA trabajó este mes todavía. Cada chat, ronda, mejora y revisión queda anotada aquí.</Empty>
      </Panel>
    );
  return (
    <Panel title="Quién hizo el trabajo" aside={`este mes · ${percent(share.free_pct)} gratis`} flush>
      <table className="table">
        <thead>
          <tr>
            <th>IA</th>
            <th>Parte del trabajo</th>
            <th className="num">Llamadas</th>
            <th className="num">Tokens</th>
            <th>En qué</th>
          </tr>
        </thead>
        <tbody>
          {share.ais.map((a) => (
            <tr key={a.id}>
              <td>
                <div className="strong">{a.name}</div>
                <Tag tone={BILLING[a.billing].tone}>{BILLING[a.billing].label}</Tag>
                {a.paused && (
                  <div className="stamp" title={a.paused.reason}>
                    <span className="brasa">Sin cuota hasta el {a.paused.until}</span> ·{" "}
                    <button type="button" className="link" onClick={() => retry(a.id)}>
                      probar ya
                    </button>
                  </div>
                )}
              </td>
              <td>
                <div className="meter">
                  <div
                    className={a.billing === "gratis" ? "meter-fill" : "meter-fill almenara"}
                    style={{ width: `${a.calls_pct}%` }}
                  />
                </div>
                <div className="stamp">{percent(a.calls_pct)} de las llamadas</div>
              </td>
              <td className="num mono">
                {a.calls.toLocaleString("es")}
                {a.errors ? (
                  <div className="stamp brasa" title={a.last_error?.text ?? undefined}>
                    {a.errors} con error
                  </div>
                ) : null}
              </td>
              <td className="num mono">
                {a.untracked_calls === a.calls ? (
                  <span className="stamp" title="Este agente no informa sus tokens">
                    no los informa
                  </span>
                ) : (
                  <>
                    {compact(a.tokens)}
                    <div className="stamp">{percent(a.tokens_pct)}</div>
                  </>
                )}
              </td>
              <td className="stamp">
                {a.tasks.map((t) => `${TASK_LABEL[t.task] ?? t.task} ${t.calls}`).join(" · ")}
                {a.last_at && <div>última {ago(a.last_at)}</div>}
                {a.last_error?.text && !a.paused && (
                  <div className="brasa" title={a.last_error.text}>
                    último error {ago(a.last_error.at)}: {clip(a.last_error.text.replace(/^terminó con error: /, ""), 90)}
                  </div>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="footnote pad" style={{ wordBreak: "normal" }}>
        La parte se mide en llamadas: cada turno de chat, ronda, mejora o revisión cuenta una. Los tokens de Claude Code incluyen
        lo que relee de su caché. «Tu cuenta» no suma al gasto: lo cubre tu plan. Una IA sin cuota queda en pausa hasta que se
        renueve, y su trabajo pasa a la siguiente de la lista.
      </p>
    </Panel>
  );
}

const number = (v: string) => v.replace(/[^0-9.]/g, "");

/** Presupuesto de una conexión: medidor y formulario para cambiarlo. */
function Budget({ c, onSaved }: { c: AiConnection; onSaved: () => void }) {
  const [value, setValue] = useState("");
  const cap = c.budget_usd;
  const share = cap ? Math.min(100, (c.spent_usd / cap) * 100) : 0;
  return (
    <tr>
      <td>
        <div className="strong">{c.name}</div>
        <div className="stamp">
          {c.provider}
          {c.model ? ` · ${c.model}` : ""}
          {c.local ? " · local, sin costo" : ""}
        </div>
      </td>
      <td>
        {cap ? (
          <>
            <div className="meter">
              <div
                className={share >= 100 ? "meter-fill brasa" : share >= 80 ? "meter-fill almenara" : "meter-fill"}
                style={{ width: `${share}%` }}
              />
            </div>
            <div className="stamp">
              {usd(c.spent_usd)} de {usd(cap)}
              {share >= 100 ? " · pasado: Atalaya usa el siguiente de la lista" : ""}
            </div>
          </>
        ) : (
          <div className="stamp">{usd(c.spent_usd)} este mes · sin presupuesto</div>
        )}
      </td>
      <td className="shrink">
        {!c.local && (
          <form
            className="inline-form budget"
            onSubmit={async (e) => {
              e.preventDefault();
              await invoke("ai_set_budget", { connectionId: c.id, usd: Number(value) > 0 ? Number(value) : null });
              setValue("");
              onSaved();
            }}
          >
            <input
              className="mono"
              inputMode="decimal"
              value={value}
              onChange={(e) => setValue(number(e.target.value))}
              placeholder={cap ? `Hoy ${cap}` : "US$ al mes"}
              aria-label={`Presupuesto de ${c.name}`}
            />
            <button className="btn" disabled={!value}>
              Fijar
            </button>
          </form>
        )}
      </td>
    </tr>
  );
}

/** Cuánto puede gastar un solo encargo: un turno del chat, una mejora, una consulta, la ronda. */
function TaskCap({ cap, onSaved }: { cap: number | null; onSaved: () => void }) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const save = async (usd: number | null) => {
    try {
      await invoke("ai_set_task_cap", { usd });
      setValue("");
      setError(null);
      onSaved();
    } catch (e) {
      setError(String(e));
    }
  };
  return (
    <Panel title="Tope por encargo" aside={cap ? usd(cap) : "Sin tope"}>
      <p className="stamp">
        Lo máximo que puede gastar un solo encargo. Claude Code se detiene al llegar (lo revisa entre un paso y otro, así que un
        paso largo puede pasarlo un poco). Los modelos por API con precio conocido acortan su respuesta para no pasarlo, y si solo
        leer ya costaría más, no llaman. Los agentes de terminal no informan su gasto mientras trabajan: a ellos los limita el
        tiempo.
      </p>
      <form
        className="inline-form budget"
        onSubmit={(e) => {
          e.preventDefault();
          save(Number(value) > 0 ? Number(value) : null);
        }}
      >
        <input
          className="mono"
          inputMode="decimal"
          value={value}
          onChange={(e) => setValue(number(e.target.value))}
          placeholder={cap ? `Hoy ${cap}` : "US$ por encargo"}
          aria-label="Tope por encargo"
        />
        <button className="btn" disabled={!value}>
          Fijar
        </button>
        {cap && (
          <button type="button" className="link" onClick={() => save(null)}>
            Quitar
          </button>
        )}
      </form>
      {error && <p className="tablero-error">{error}</p>}
    </Panel>
  );
}

/** Precio de un modelo en dólares por millón de tokens. Nunca se inventa: lo fija la persona o lo informa OpenRouter. */
function PriceForm({ row, onSaved, onCancel }: { row: AiRow; onSaved: () => void; onCancel: () => void }) {
  const [input, setInput] = useState("");
  const [output, setOutput] = useState("");
  const [error, setError] = useState<string | null>(null);
  return (
    <form
      className="inline-form"
      onSubmit={async (e) => {
        e.preventDefault();
        try {
          await invoke("ai_set_price", {
            connectionId: row.connection_id,
            model: row.model,
            input: Number(input),
            output: Number(output),
          });
          onSaved();
        } catch (err) {
          setError(String(err));
        }
      }}
    >
      <input
        className="mono"
        inputMode="decimal"
        value={input}
        onChange={(e) => setInput(number(e.target.value))}
        placeholder="Entrada"
        aria-label="Entrada por millón"
      />
      <input
        className="mono"
        inputMode="decimal"
        value={output}
        onChange={(e) => setOutput(number(e.target.value))}
        placeholder="Salida"
        aria-label="Salida por millón"
      />
      <button className="btn" disabled={!input || !output}>
        Guardar
      </button>
      <button type="button" className="link" onClick={onCancel}>
        Cancelar
      </button>
      {error && <span className="stamp brasa">{error}</span>}
    </form>
  );
}

/** Lo que gastó cada IA conectada, según el registro de Atalaya: cada llamada queda anotada. */
function IaRegistradas() {
  const [data, setData] = useState<AiUsage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pricing, setPricing] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await invoke<AiUsage>("ai_usage"));
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  if (error) return <div className="banner">{error}</div>;
  if (!data) return null;
  if (!data.connections.length && !data.by_model.length)
    return (
      <>
        <Panel title="IA conectadas">
          <Empty>
            Todavía no hay IA conectadas. Agrégalas en Conexiones → IA (Anthropic, OpenAI, Gemini, compatibles con OpenAI o
            locales) o Agentes. Cada llamada que haga Atalaya queda anotada aquí, con sus tokens y su costo.
          </Empty>
        </Panel>
        <TaskCap cap={data.task_cap_usd} onSaved={load} />
      </>
    );

  const calls = data.by_model.reduce((n, r) => n + r.calls, 0);
  const tokens = data.by_model.reduce((n, r) => n + r.input_tokens + r.output_tokens, 0);
  const unpriced = data.by_model.reduce((n, r) => n + r.unpriced_calls, 0);
  const key = (r: AiRow) => `${r.connection_id}/${r.model}`;

  return (
    <>
      <div className="tiles">
        <div className="tile">
          <div className="tile-label">Gasto del mes</div>
          <div className="tile-value">{usd(data.month_usd)}</div>
          <div className="stamp">{unpriced ? "solo lo que tiene precio" : "todas las IA"}</div>
        </div>
        <div className="tile">
          <div className="tile-label">Llamadas</div>
          <div className="tile-value">{calls.toLocaleString("es")}</div>
        </div>
        <div className="tile">
          <div className="tile-label">Tokens</div>
          <div className="tile-value">{compact(tokens)}</div>
        </div>
        <div className="tile">
          <div className="tile-label">Hecho gratis</div>
          <div className="tile-value">{percent(data.by_ai.free_pct)}</div>
          <div className="stamp">de las llamadas</div>
        </div>
        <div className="tile">
          <div className="tile-label">Sin precio</div>
          <div className={`tile-value ${unpriced ? "almenara" : ""}`}>{unpriced.toLocaleString("es")}</div>
          <div className="stamp">llamadas</div>
        </div>
      </div>

      <Reparto share={data.by_ai} onChanged={load} />

      <Panel title="Por modelo" aside="este mes" flush>
        {data.by_model.length ? (
          <table className="table">
            <thead>
              <tr>
                <th>Conexión y modelo</th>
                <th className="num">Llamadas</th>
                <th className="num">Entrada</th>
                <th className="num">Salida</th>
                <th className="num">Costo</th>
              </tr>
            </thead>
            <tbody>
              {data.by_model.map((r) => (
                <tr key={key(r)}>
                  <td>
                    <div className="strong">{r.connection}</div>
                    <div className="stamp mono">{r.model}</div>
                    {pricing === key(r) && (
                      <PriceForm
                        row={r}
                        onCancel={() => setPricing(null)}
                        onSaved={async () => {
                          setPricing(null);
                          await load();
                        }}
                      />
                    )}
                  </td>
                  <td className="num mono">
                    {r.calls.toLocaleString("es")}
                    {r.errors ? <div className="stamp brasa">{r.errors} con error</div> : null}
                  </td>
                  <td className="num mono">{compact(r.input_tokens)}</td>
                  <td className="num mono">{compact(r.output_tokens)}</td>
                  <td className="num mono">
                    {r.billing === "cuenta" && <div className="stamp">tu cuenta</div>}
                    {r.billing === "gratis" && r.cost_usd === null && <div className="stamp">gratis</div>}
                    {r.cost_usd !== null && <div>{usd(r.cost_usd)}</div>}
                    {r.unpriced_calls > 0 && (
                      <button
                        className="link almenara"
                        title="Fija el precio por millón de tokens"
                        onClick={() => setPricing(key(r))}
                      >
                        precio sin definir
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <div className="panel-body">
            <Empty>Ninguna llamada este mes.</Empty>
          </div>
        )}
      </Panel>

      <TaskCap cap={data.task_cap_usd} onSaved={load} />

      <div className="split">
        <Panel title="Presupuestos" aside="por conexión" flush>
          {data.connections.length ? (
            <table className="table">
              <tbody>
                {data.connections.map((c) => (
                  <Budget key={c.id} c={c} onSaved={load} />
                ))}
              </tbody>
            </table>
          ) : (
            <div className="panel-body">
              <Empty>Sin IA conectadas.</Empty>
            </div>
          )}
        </Panel>

        <Panel title="Por tarea" aside="este mes" flush>
          {data.by_task.length ? (
            <table className="table">
              <tbody>
                {data.by_task.map((t) => (
                  <tr key={t.task}>
                    <td>{TASK_LABEL[t.task] ?? t.task}</td>
                    <td className="num mono">{t.calls.toLocaleString("es")} llamadas</td>
                    <td className="num mono">{compact(t.tokens)}</td>
                    <td className="num mono">{t.cost_usd !== null ? usd(t.cost_usd) : "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <div className="panel-body">
              <Empty>Ninguna tarea este mes.</Empty>
            </div>
          )}
        </Panel>
      </div>

      {data.prices.length > 0 && (
        <Panel title="Precios conocidos" aside="US$ por millón de tokens" flush>
          <table className="table">
            <thead>
              <tr>
                <th>Modelo</th>
                <th className="num">Entrada</th>
                <th className="num">Salida</th>
                <th>Origen</th>
              </tr>
            </thead>
            <tbody>
              {data.prices.map((p) => (
                <tr key={`${p.provider}/${p.model}`}>
                  <td>
                    <div className="mono">{p.model}</div>
                    <div className="stamp">{p.provider}</div>
                  </td>
                  <td className="num mono">{p.input_per_mtok}</td>
                  <td className="num mono">{p.output_per_mtok}</td>
                  <td className="stamp">
                    {p.source === "manual" ? "Fijado a mano" : p.source === "openrouter" ? "Informado por OpenRouter" : p.source}{" "}
                    · {ago(p.updated_at)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </Panel>
      )}
    </>
  );
}

/** Gasto y uso de cada IA: el registro propio de Atalaya y, si está conectada, Claude Platform. */
export function Consumo() {
  const [status, setStatus] = useState<ClaudeStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [budget, setBudget] = useState("");

  const load = useCallback(async (command: string, args: Record<string, unknown> = {}) => {
    setBusy(true);
    try {
      setStatus(await invoke<ClaudeStatus>(command, args));
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    load("claude_status");
  }, [load]);

  const summary = status?.summary ?? null;
  const cap = status?.budget_usd ?? null;
  const spent = summary?.month_usd ?? 0;
  const share = cap ? Math.min(100, (spent / cap) * 100) : 0;
  const peak = Math.max(...(summary?.days.map((d) => d.usd) ?? [0]), 0.01);

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Consumo</h1>
          <p>
            Lo que gastó cada IA este mes. Atalaya anota cada llamada que hace: conexión, modelo, tarea y tokens. Los precios no
            se inventan: si no se conocen, se muestran los tokens y la marca «precio sin definir».
          </p>
        </div>
      </header>

      <IaRegistradas />

      <header className="page-head">
        <div>
          <h2>Claude Platform</h2>
          <p>
            Gasto de la API de Claude según la consola de Anthropic, para cuadrar los números. No incluye lo que consumes con tu
            suscripción de Claude Code, que no pasa por la API.
          </p>
        </div>
        <div className="actions">
          {summary && <span className="stamp">Dato de {ago(summary.fetched_at)}</span>}
          <button className="btn" disabled={busy || !status?.connected} onClick={() => load("claude_refresh")}>
            {busy ? "Consultando…" : "Consultar ahora"}
          </button>
        </div>
      </header>

      {(error || status?.error) && <div className="banner">{error ?? status?.error}</div>}

      {status && !status.connected ? (
        <Panel title="Conectar Claude Platform" aside={<Tag tone="almenara">Falta la llave</Tag>}>
          <ol className="steps">
            <li>
              En la consola de Claude, crea una organización si tu cuenta es individual: Settings → Organization. La API de
              administración no está disponible para cuentas individuales.
            </li>
            <li>Crea una llave de administrador: Settings → Admin keys. Empieza con sk-ant-admin.</li>
            <li>Agrégala en Conexiones → IA → «Claude Platform (consumo)». Se guarda en el Llavero de macOS.</li>
          </ol>
        </Panel>
      ) : summary ? (
        <>
          <div className="tiles">
            <div className="tile">
              <div className="tile-label">Gasto del mes</div>
              <div className={`tile-value ${cap && share >= 80 ? "almenara" : ""}`}>{usd(spent)}</div>
              <div className="stamp">{summary.month}</div>
            </div>
            <div className="tile">
              <div className="tile-label">Solicitudes</div>
              <div className="tile-value">{summary.requests.toLocaleString("es")}</div>
            </div>
            <div className="tile">
              <div className="tile-label">Tokens de entrada</div>
              <div className="tile-value">{compact(summary.input_tokens)}</div>
            </div>
            <div className="tile">
              <div className="tile-label">Tokens de salida</div>
              <div className="tile-value">{compact(summary.output_tokens)}</div>
            </div>
          </div>

          <div className="split">
            <Panel title="Presupuesto mensual" aside={cap ? `${Math.round(share)} % usado` : "Sin presupuesto"}>
              {cap ? (
                <>
                  <div className="meter">
                    <div
                      className={share >= 100 ? "meter-fill brasa" : share >= 80 ? "meter-fill almenara" : "meter-fill"}
                      style={{ width: `${share}%` }}
                    />
                  </div>
                  <p className="footnote">
                    {usd(spent)} de {usd(cap)}. Atalaya avisa al llegar al 80 % y al 100 %.
                  </p>
                </>
              ) : (
                <Empty>Fija un tope para recibir un aviso antes de pasarte.</Empty>
              )}
              <form
                className="inline-form budget"
                onSubmit={(e) => {
                  e.preventDefault();
                  load("claude_set_budget", { usd: Number(budget) });
                  setBudget("");
                }}
              >
                <input
                  className="mono"
                  inputMode="decimal"
                  value={budget}
                  onChange={(e) => setBudget(e.target.value.replace(/[^0-9.]/g, ""))}
                  placeholder={cap ? `Cambiar (hoy ${cap})` : "Dólares al mes"}
                />
                <button className="btn" disabled={!budget || busy}>
                  Fijar
                </button>
              </form>
            </Panel>

            <Panel title="Gasto por día" aside="últimos 14 días">
              {summary.days.length ? (
                <div className="bars">
                  {summary.days.map((d) => (
                    <div className="bar" key={d.day} title={`${d.day}: ${usd(d.usd)}`}>
                      <div className="bar-fill" style={{ height: `${Math.max(2, (d.usd / peak) * 100)}%` }} />
                      <span className="stamp">{d.day.slice(8)}</span>
                    </div>
                  ))}
                </div>
              ) : (
                <Empty>Sin gasto registrado en estos días.</Empty>
              )}
            </Panel>
          </div>

          <Panel title="Por modelo" aside={`${summary.models.length} modelos`} flush>
            {summary.models.length ? (
              <table className="table">
                <thead>
                  <tr>
                    <th>Modelo</th>
                    <th className="num">Gasto</th>
                    <th className="num">Solicitudes</th>
                    <th className="num">Entrada</th>
                    <th className="num">Salida</th>
                  </tr>
                </thead>
                <tbody>
                  {summary.models.map((m) => (
                    <tr key={m.model}>
                      <td className="mono">{m.model}</td>
                      <td className="num mono">{usd(m.usd)}</td>
                      <td className="num mono">{m.requests.toLocaleString("es")}</td>
                      <td className="num mono">{compact(m.input_tokens)}</td>
                      <td className="num mono">{compact(m.output_tokens)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            ) : (
              <div className="panel-body">
                <Empty>Sin uso de la API este mes.</Empty>
              </div>
            )}
          </Panel>
        </>
      ) : (
        status && <Empty>Llave guardada. Pulsa “Consultar ahora” para leer el consumo.</Empty>
      )}
    </div>
  );
}
