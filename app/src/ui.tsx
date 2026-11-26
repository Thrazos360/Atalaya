import type { ReactNode } from "react";
import type { Status } from "./api";

/** La atalaya: torre en color de texto y almenara encendida. */
export function Logo({ size = 22 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 64 64" aria-hidden>
      <path d="M23 13.5a9 5.5 0 0 1 18 0z" fill="currentColor" />
      <circle cx="29" cy="20.5" r="3.4" fill="var(--almenara)" />
      <path d="M34 20.5 47 16.2a2 2 0 0 1 2.6 1.9v4.8a2 2 0 0 1-2.6 1.9z" fill="var(--almenara)" />
      <rect x="21" y="26" width="22" height="4" rx="1" fill="currentColor" />
      <path d="M24 30h16l4.2 24H19.8z" fill="currentColor" />
      <rect x="17" y="54" width="30" height="3.5" rx="1" fill="currentColor" />
    </svg>
  );
}

const ICONS: Record<string, ReactNode> = {
  vigia: (
    <>
      <path d="M1.5 8S4 3.5 8 3.5 14.5 8 14.5 8 12 12.5 8 12.5 1.5 8 1.5 8Z" />
      <circle cx="8" cy="8" r="2" />
    </>
  ),
  alertas: <path d="M4 11V7a4 4 0 0 1 8 0v4l1.5 1.5h-11L4 11Zm2.5 3h3" />,
  proyectos: <path d="M1.5 4.5v8h13v-7h-6l-1.5-2h-5.5v1Z" />,
  mejoras: <path d="m1.5 12 4-4 3 2.5 6-6.5m0 0h-3.5m3.5 0v3.5" />,
  ajustes: <path d="M2 4.5h6m3 0h3M2 11.5h3m3 0h6M9.5 3v3m-3 4v3" />,
  decisiones: <path d="M2 4h7M2 8h5M2 12h7m2.5-6 1.5 1.5L15 5" />,
  radar: (
    <>
      <circle cx="8" cy="8" r="6" />
      <circle cx="8" cy="8" r="2.5" />
      <path d="M8 8 12.5 4" />
    </>
  ),
  agente: <path d="M2 3.5h12v9H2v-9Zm2.5 3 2 1.5-2 1.5M8 10h3" />,
  consumo: <path d="M2 13.5h12M4 11V7m4 4V3.5M12 11V8.5" />,
  tareas: (
    <>
      <circle cx="8" cy="8" r="6" />
      <path d="M8 4.5V8l2.5 1.5" />
    </>
  ),
  conexiones: (
    <>
      <circle cx="4" cy="4" r="2" />
      <circle cx="12" cy="12" r="2" />
      <circle cx="12" cy="4" r="2" />
      <path d="M6 4h4M12 6v4M5.5 5.5l5 5" />
    </>
  ),
  skills: <path d="M8 1.5 9.6 6.4 14.5 8 9.6 9.6 8 14.5 6.4 9.6 1.5 8 6.4 6.4Z" />,
  ia: (
    <>
      <rect x="3.5" y="3.5" width="9" height="9" rx="2" />
      <path d="M6 1.5v2M10 1.5v2M6 12.5v2M10 12.5v2M1.5 6h2M1.5 10h2M12.5 6h2M12.5 10h2M6.5 6.5h3v3h-3z" />
    </>
  ),
  contraer: <path d="M2 3.5h12v9H2v-9Zm4 0v9m5-6-2 1.5 2 1.5" />,
  expandir: <path d="M2 3.5h12v9H2v-9Zm4 0v9m3-6 2 1.5-2 1.5" />,
};

export function Icon({ name }: { name: string }) {
  return (
    <svg
      className="icon"
      width="15"
      height="15"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      {ICONS[name]}
    </svg>
  );
}

const TONE: Record<Status, string> = { ok: "liquen", aviso: "almenara", falla: "brasa" };
const LABEL: Record<Status, string> = { ok: "En orden", aviso: "Aviso", falla: "Falla" };

/** Punto de estado: verde en orden, ámbar aviso, rojo falla, gris sin datos. */
export function Pip({ status }: { status?: Status }) {
  return <span className={`pip ${status ? TONE[status] : ""}`} />;
}

/** Etiqueta con borde, en mayúsculas, como las marcas de un instrumento. */
export function Tag({ tone, children }: { tone?: "liquen" | "almenara" | "brasa"; children: ReactNode }) {
  return <span className={`tag ${tone ?? ""}`}>{children}</span>;
}

export function StatusTag({ status }: { status?: Status }) {
  if (!status) return <Tag>Sin revisar</Tag>;
  return <Tag tone={TONE[status] as "liquen"}>{LABEL[status]}</Tag>;
}

/** Módulo con marco: barra de título fija y contenido. */
export function Panel({
  title,
  aside,
  children,
  flush,
}: {
  title: ReactNode;
  aside?: ReactNode;
  children: ReactNode;
  /** Sin relleno interior, para tablas que llegan al borde. */
  flush?: boolean;
}) {
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>{title}</h2>
        {aside && <div className="panel-aside">{aside}</div>}
      </header>
      <div className={flush ? "panel-body flush" : "panel-body"}>{children}</div>
    </section>
  );
}

export function Empty({ children }: { children: ReactNode }) {
  return <p className="empty-note">{children}</p>;
}

/** Claude responde con algo de Markdown; aquí solo se resaltan las **negritas** y el `código`. */
export function Rich({ text }: { text: string }) {
  return (
    <>
      {text
        .split(/(\*\*[^*]+\*\*|`[^`]+`)/)
        .map((part, i) =>
          part.startsWith("**") ? (
            <b key={i}>{part.slice(2, -2)}</b>
          ) : part.startsWith("`") ? (
            <code key={i}>{part.slice(1, -1)}</code>
          ) : (
            part
          ),
        )}
    </>
  );
}
