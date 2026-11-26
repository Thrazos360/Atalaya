import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type Context, type Improvement, type SystemStatus, alertsOf, api } from "./api";
import { Icon, Logo } from "./ui";
import { Vigia } from "./views/Vigia";
import { Alertas } from "./views/Alertas";
import { Proyectos } from "./views/Proyectos";
import { Mejoras } from "./views/Mejoras";
import { Ajustes } from "./views/Ajustes";
import { Agente, type AgentActivity } from "./views/Agente";
import { Consumo } from "./views/Consumo";
import { Decisiones } from "./views/Decisiones";
import { Radar } from "./views/Radar";
import { Conexiones } from "./views/Conexiones";
import { Bienvenida } from "./views/Bienvenida";
import { IA } from "./views/IA";
import { Skills } from "./views/Skills";
import { Tareas } from "./views/Tareas";
import "./styles.css";

export type View =
  | "vigia"
  | "decisiones"
  | "alertas"
  | "proyectos"
  | "mejoras"
  | "radar"
  | "tareas"
  | "agente"
  | "consumo"
  | "conexiones"
  | "ia"
  | "skills"
  | "bienvenida"
  | "ajustes";

/** Lo que cada pantalla recibe de la consola. */
export type Console = {
  contexts: Context[];
  improvements: Improvement[];
  system: SystemStatus | null;
  /** Vuelve a leer todo. */
  refresh: () => Promise<void>;
  /** Abre el detalle de un proyecto. */
  openProject: (slug: string) => void;
  go: (view: View) => void;
  /** Lleva una mejora trabada al Agente, para responderle y que Claude Code la termine. */
  answerInAgent: (id: number) => void;
};

function App() {
  const [view, setView] = useState<View>("vigia");
  const [project, setProject] = useState<string | null>(null);
  const [contexts, setContexts] = useState<Context[]>([]);
  const [improvements, setImprovements] = useState<Improvement[]>([]);
  const [system, setSystem] = useState<SystemStatus | null>(null);
  // La mejora que espera tu respuesta en el Agente.
  const [handoff, setHandoff] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [updated, setUpdated] = useState<Date | null>(null);
  const [slow, setSlow] = useState(false);
  // La barra lateral se contrae a mano (se recuerda) o sola cuando la ventana es angosta.
  const [collapsed, setCollapsed] = useState(() => {
    try {
      return localStorage.getItem("barra") === "contraida";
    } catch {
      return false;
    }
  });
  const [narrow, setNarrow] = useState(() => window.matchMedia("(max-width: 1000px)").matches);

  const toggleSidebar = useCallback(() => {
    setCollapsed((was) => {
      try {
        localStorage.setItem("barra", was ? "abierta" : "contraida");
      } catch {
        // Sin almacenamiento, la preferencia vale solo para esta sesión.
      }
      return !was;
    });
  }, []);

  useEffect(() => {
    const query = window.matchMedia("(max-width: 1000px)");
    const onWidth = () => setNarrow(query.matches);
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "b") {
        event.preventDefault();
        toggleSidebar();
      }
    };
    query.addEventListener("change", onWidth);
    window.addEventListener("keydown", onKey);
    return () => {
      query.removeEventListener("change", onWidth);
      window.removeEventListener("keydown", onKey);
    };
  }, [toggleSidebar]);

  const refresh = useCallback(async () => {
    try {
      const [all, list, status] = await Promise.all([api.overview(), api.improvements(), api.systemStatus()]);
      setContexts(all);
      setImprovements(list);
      setSystem(status);
      setError(null);
      setUpdated(new Date());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoaded(true);
    }
  }, []);

  // Todo se lee en vivo: al abrir, al volver a la ventana y cada minuto.
  useEffect(() => {
    refresh();
    window.addEventListener("focus", refresh);
    const timer = setInterval(refresh, 60_000);
    return () => {
      window.removeEventListener("focus", refresh);
      clearInterval(timer);
    };
  }, [refresh]);

  // Al abrir Atalaya desde una notificación, se va a la vista que la originó.
  useEffect(() => {
    const NOTICE_VIEWS: View[] = ["decisiones", "mejoras", "alertas", "radar", "consumo"];
    const follow = () =>
      api
        .takeNotice()
        .then((target) => {
          // «agente:30»: la mejora 30 necesita tu respuesta; se responde en el Agente.
          const asked = target?.match(/^agente:(\d+)$/);
          if (asked) {
            setHandoff(Number(asked[1]));
            setView("agente");
          } else if (target && (NOTICE_VIEWS as string[]).includes(target)) {
            setView(target as View);
            setProject(null);
          }
        })
        .catch(() => {});
    // Al abrir la app, y cuando macOS la reabre (el clic en la notificación, o en el Dock).
    follow();
    const stop = listen("reabrir", follow);
    return () => {
      stop.then((off) => off());
    };
  }, []);

  // Si la primera lectura tarda, se reintenta y se explica la causa más probable.
  useEffect(() => {
    if (loaded) return;
    const hint = setTimeout(() => setSlow(true), 6000);
    const retry = setInterval(refresh, 4000);
    return () => {
      clearTimeout(hint);
      clearInterval(retry);
    };
  }, [loaded, refresh]);

  // Sin proyectos al abrir, se empieza por la bienvenida.
  const [welcomed, setWelcomed] = useState(false);
  useEffect(() => {
    if (!loaded || welcomed) return;
    setWelcomed(true);
    if (!error && contexts.length === 0) setView("bienvenida");
  }, [loaded, welcomed, error, contexts.length]);

  // Decisiones pendientes, para el contador de la barra.
  const [decisions, setDecisions] = useState(0);
  useEffect(() => {
    invoke<{ decisions: { status: string }[] }>("list_decisions")
      .then((d) => setDecisions(d.decisions.filter((x) => x.status === "pendiente").length))
      .catch(() => {});
  }, [updated, view]);

  // Al abrir, se comprueba el sistema: si falta un permiso, macOS lo pide ahora y no a mitad de un trabajo.
  const [blocked, setBlocked] = useState<string[]>([]);
  useEffect(() => {
    invoke<{ name: string; ok: boolean }[]>("doctor")
      .then((items) => setBlocked(items.filter((i) => !i.ok && i.name.startsWith("Acceso")).map((i) => i.name)))
      .catch(() => {});
  }, []);

  const open = alertsOf(contexts).filter((a) => !a.acked);
  const proposed = improvements.filter((i) => i.status === "propuesta").length;

  const [activity, setActivity] = useState<AgentActivity>({ busy: false, waiting: 0 });
  const [agentOpened, setAgentOpened] = useState(false);
  useEffect(() => {
    if (view === "agente") setAgentOpened(true);
  }, [view]);

  const go = (next: View) => {
    setView(next);
    if (next !== "proyectos") setProject(null);
  };
  const shared: Console = {
    contexts,
    improvements,
    system,
    refresh,
    go,
    openProject: (slug) => {
      setProject(slug);
      setView("proyectos");
    },
    answerInAgent: (id) => {
      setHandoff(id);
      setView("agente");
    },
  };

  const NAV: { id: View; label: string; badge?: React.ReactNode }[] = [
    { id: "decisiones", label: "Decisiones", badge: decisions ? <span className="count hot">{decisions}</span> : null },
    { id: "vigia", label: "Vigía", badge: <span className={open.length ? "pip almenara" : "pip liquen"} /> },
    { id: "alertas", label: "Alertas", badge: open.length ? <span className="count hot">{open.length}</span> : null },
    { id: "proyectos", label: "Proyectos", badge: <span className="count">{contexts.length}</span> },
    { id: "mejoras", label: "Mejoras", badge: proposed ? <span className="count hot">{proposed}</span> : null },
    { id: "radar", label: "Radar" },
    { id: "tareas", label: "Tareas" },
    {
      id: "agente",
      label: "Agente",
      badge: activity.waiting ? (
        <span className="count hot" title="Espera tu visto bueno">
          {activity.waiting}
        </span>
      ) : activity.busy ? (
        <span className="pip almenara" title="Trabajando" />
      ) : null,
    },
    { id: "consumo", label: "Consumo" },
    { id: "conexiones", label: "Conexiones" },
    { id: "ia", label: "IA" },
    { id: "skills", label: "Skills" },
    { id: "ajustes", label: "Ajustes" },
  ];

  const guard = system?.service_running;

  return (
    <div className={collapsed || narrow ? "shell collapsed" : "shell"}>
      <aside className="sidebar">
        <div className="drag" data-tauri-drag-region />
        <div className="brand">
          <Logo />
          <span className="brand-name">atalaya</span>
          {system && <span className="version">v{system.version}</span>}
        </div>
        <nav>
          {NAV.map((item) => (
            <button
              key={item.id}
              className={view === item.id ? "nav active" : "nav"}
              title={item.label}
              onClick={() => go(item.id)}
            >
              <Icon name={item.id} />
              <span className="nav-label">{item.label}</span>
              {item.badge}
            </button>
          ))}
        </nav>
        {!narrow && (
          <button
            className="nav collapse"
            title={collapsed ? "Expandir la barra (⌘B)" : "Contraer la barra (⌘B)"}
            onClick={toggleSidebar}
          >
            <Icon name={collapsed ? "expandir" : "contraer"} />
            <span className="nav-label">Contraer</span>
            <span className="soon">⌘B</span>
          </button>
        )}
        <div className="guard">
          <span className={guard ? "pip liquen" : guard === false ? "pip brasa" : "pip"} />
          <div className="guard-text">
            <div className="guard-title">{guard ? "En guardia" : guard === false ? "Vigía detenido" : "Vigía"}</div>
            <div className="guard-sub">{guard ? "Servicio en este Mac" : "Sin servicio en segundo plano"}</div>
          </div>
        </div>
      </aside>

      <div className="stage">
        <header className="topbar" data-tauri-drag-region>
          <div className="topbar-summary">
            <span className={open.length ? "pip almenara" : "pip liquen"} />
            {open.length ? (
              <>
                <b>{open.length}</b> {open.length === 1 ? "alerta abierta" : "alertas abiertas"}
              </>
            ) : (
              "Sin alertas abiertas"
            )}
            {proposed > 0 && (
              <>
                {" · "}
                <b>{proposed}</b> {proposed === 1 ? "mejora propuesta" : "mejoras propuestas"}
              </>
            )}
          </div>
          <div className="topbar-right">
            {updated && (
              <span className="stamp">
                Dato de las {updated.toLocaleTimeString("es", { hour: "2-digit", minute: "2-digit" })}
              </span>
            )}
            <button className="btn" onClick={refresh}>
              Actualizar
            </button>
          </div>
        </header>

        <main className={view === "agente" ? "canvas fixed" : "canvas"}>
          {error && <div className="banner">{error}</div>}
          {blocked.length > 0 && (
            <div className="banner">
              Atalaya no puede leer {blocked.length === 1 ? "una carpeta" : `${blocked.length} carpetas`} de tus proyectos (
              {blocked.join(", ")}). Dale acceso en Ajustes del Sistema → Privacidad y seguridad → Archivos y carpetas.
            </div>
          )}
          {!loaded ? (
            <p className="empty-note">
              Cargando…
              {slow &&
                " Está tardando más de lo normal. Si macOS muestra un aviso pidiendo acceso a tus carpetas, acéptalo para que Atalaya pueda leer tus proyectos."}
            </p>
          ) : view === "vigia" ? (
            <Vigia {...shared} />
          ) : view === "alertas" ? (
            <Alertas {...shared} />
          ) : view === "proyectos" ? (
            <Proyectos {...shared} selected={project} onBack={() => setProject(null)} />
          ) : view === "mejoras" ? (
            <Mejoras {...shared} />
          ) : view === "agente" ? null : view === "consumo" ? (
            <Consumo />
          ) : view === "decisiones" ? (
            <Decisiones {...shared} />
          ) : view === "radar" ? (
            <Radar {...shared} />
          ) : view === "tareas" ? (
            <Tareas {...shared} />
          ) : view === "conexiones" ? (
            <Conexiones />
          ) : view === "ia" ? (
            <IA {...shared} />
          ) : view === "skills" ? (
            <Skills {...shared} />
          ) : view === "bienvenida" ? (
            <Bienvenida {...shared} />
          ) : (
            <Ajustes {...shared} />
          )}
          {/* El Agente queda montado desde la primera visita: al cambiar de vista, la conversación y el trabajo siguen. */}
          {loaded && agentOpened && (
            <Agente
              {...shared}
              visible={view === "agente"}
              onActivity={setActivity}
              handoff={improvements.find((i) => i.id === handoff) ?? null}
              onHandoffTaken={() => setHandoff(null)}
            />
          )}
        </main>
      </div>
    </div>
  );
}

export default App;
