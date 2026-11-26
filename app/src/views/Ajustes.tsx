import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Console } from "../App";
import { KIND_LABEL, everyLabel } from "../api";
import { Empty, Panel, Pip, Tag } from "../ui";


type Item = { name: string; ok: boolean; detail: string; fix: string };

/** Todo lo que Atalaya necesita para no trabarse: permisos, herramientas, llaves y servicio. */
function Doctor() {
  const [items, setItems] = useState<Item[] | null>(null);
  const run = () => {
    setItems(null);
    invoke<Item[]>("doctor").then(setItems).catch(() => setItems([]));
  };
  useEffect(run, []);
  const missing = items?.filter((i) => !i.ok) ?? [];
  return (
    <Panel
      title="Comprobación del sistema"
      aside={
        <button className="link" onClick={run}>
          Comprobar de nuevo
        </button>
      }
      flush
    >
      {items === null ? (
        <div className="panel-body">
          <Empty>Comprobando…</Empty>
        </div>
      ) : (
        <table className="table">
          <tbody>
            {[...missing, ...items.filter((i) => i.ok)].map((item) => (
              <tr key={item.name}>
                <td className="shrink">
                  <span className="cell-name">
                    <Pip status={item.ok ? "ok" : item.fix.startsWith("Opcional") ? "aviso" : "falla"} />
                    {item.name}
                  </span>
                </td>
                <td>
                  <span className="mono selectable">{item.detail}</span>
                  {!item.ok && <div className="stamp">{item.fix}</div>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </Panel>
  );
}

export function Ajustes({ system, contexts, refresh, go }: Console) {
  if (!system) return <Empty>No se pudo leer el estado del sistema.</Empty>;
  const { email } = system;

  return (
    <div className="stack">
      <header className="page-head">
        <div>
          <h1>Ajustes del sistema</h1>
          <p>Cómo está configurado este nodo de Atalaya. Las llaves nunca se muestran aquí.</p>
        </div>
        <Tag>v{system.version}</Tag>
      </header>

      <div className="split">
        <Panel
          title="Nodo y rondas"
          aside={
            <span className={system.service_running ? "liquen" : "brasa"}>
              {system.service_running ? "Vigía en guardia" : "Vigía detenido"}
            </span>
          }
        >
          <ul className="kv">
            <li>
              <span>Servicio en segundo plano</span>
              <span className="cell-name">
                <Pip status={system.service_running ? "ok" : "falla"} />
                {system.service_running ? "Activo en este Mac" : "No está corriendo"}
              </span>
            </li>
            <li>
              <span>Proyectos vigilados</span>
              <span className="mono">{contexts.length}</span>
            </li>
            {system.rounds.map((round) => (
              <li key={round.kind}>
                <span>Ronda de {(KIND_LABEL[round.kind] ?? round.kind).toLowerCase()}</span>
                <span className="mono">{everyLabel(round.every_seconds)}</span>
              </li>
            ))}
          </ul>
          <p className="footnote selectable">Base de datos: {system.database}</p>
        </Panel>

        <Panel
          title="Conexiones"
          aside={
            <button className="link" onClick={() => go("conexiones")}>
              Abrir Conexiones
            </button>
          }
        >
          <p>
            Repositorios, hosting, bases de datos, avisos e IA se agregan, prueban y editan en{" "}
            <button className="link" onClick={() => go("conexiones")}>
              Conexiones
            </button>
            . Puede haber varias del mismo proveedor y cada proyecto elige las suyas en su ficha.
          </p>
          <p className="footnote">
            Las llaves viven en el Llavero de macOS, no en la base de datos ni en archivos. Desde la terminal:{" "}
            <span className="mono selectable">atalaya connections list</span>.
          </p>
          <p>
            Quién construye, revisa, valida y conversa se elige en{" "}
            <button className="link" onClick={() => go("ia")}>
              IA
            </button>
            .
          </p>
        </Panel>
      </div>

      <div className="split">
        <Panel
          title="Correo"
          aside={<span className={email.ready ? "liquen" : "almenara"}>{email.ready ? "Listo" : "Sin configurar"}</span>}
        >
          <ul className="kv">
            <li>
              <span>Cuenta que envía</span>
              <span className="mono selectable">{email.address ?? "—"}</span>
            </li>
            <li>
              <span>Destinatario</span>
              <span className="mono selectable">{email.to ?? "la misma cuenta"}</span>
            </li>
            <li>
              <span>Resumen diario</span>
              <span className="mono">{String(email.report_hour).padStart(2, "0")}:00 hora local</span>
            </li>
            <li>
              <span>Alertas</span>
              <span>Un correo por ronda, solo cuando algo cambia</span>
            </li>
          </ul>
          <p className="footnote">
            Se configura y se prueba en Conexiones → Avisos → Correo (sirve Gmail o cualquier servidor SMTP).
          </p>
        </Panel>

        <Panel
          title="Interruptores"
          aside={<span className={system.flags_store ? "liquen" : "almenara"}>{system.flags_store ? "Almacén creado" : "Sin almacén"}</span>}
        >
          <ul className="kv">
            <li>
              <span>Dónde viven</span>
              <span>Global Config de Vercel</span>
            </li>
            <li>
              <span>Almacén</span>
              <span className="mono selectable">{system.flags_store ?? "se crea al usar el primer interruptor"}</span>
            </li>
            <li>
              <span>Tiempo de reacción</span>
              <span>Hasta 1 minuto, sin desplegar</span>
            </li>
          </ul>
          <p className="footnote">
            Una app nueva se conecta con <span className="mono selectable">atalaya flags connect &lt;proyecto&gt;</span>.
          </p>
        </Panel>
      </div>
      <Panel
        title="Autonomía del agente"
        aside={<span className={system.auto_merge ? "almenara" : ""}>{system.auto_merge ? "Llega sola a producción" : "Espera tu revisión"}</span>}
      >
        <ul className="flags">
          <li>
            <button
              role="switch"
              aria-checked={system.auto_merge}
              aria-label="Fusionar sola cuando las comprobaciones pasen"
              className={system.auto_merge ? "switch on" : "switch"}
              onClick={async () => {
                await invoke("set_auto_merge", { on: !system.auto_merge });
                await refresh();
              }}
            />
            <div className="flag-text">
              <div className="strong">Llevar a producción sin esperar mi revisión</div>
              <div className="flag-desc">
                Cuando el agente termina una mejora y todas las comprobaciones de su pull request pasan, Atalaya lo fusiona
                sola y el cambio sale a producción. Si alguna comprobación falla, o el repo no tiene ninguna, la mejora queda
                en revisión como hasta ahora.
              </div>
            </div>
            <span className={system.auto_merge ? "flag-state liquen" : "flag-state"}>{system.auto_merge ? "Activado" : "Desactivado"}</span>
            <span />
          </li>
        </ul>
        <p className="footnote">
          Apagado, cada mejora espera tu clic en “Fusionar”. Encendido, tu visto bueno es el momento en que aceptas la mejora
          o pulsas “Resolver”.
        </p>
      </Panel>

      <Doctor />
    </div>
  );
}
