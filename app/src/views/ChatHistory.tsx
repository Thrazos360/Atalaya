import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { save } from "@tauri-apps/plugin-dialog";
import { ago } from "../api";

/**
 * El historial del chat (ver `conversations.rs`): cada conversación queda guardada para
 * buscarla, retomarla o exportarla a Markdown.
 */

export type SavedMessage = { role: "usuario" | "ia" | "nota"; author: string; text: string; at?: string };

export type ConversationSummary = {
  id: string;
  title: string;
  slug: string | null;
  model: string;
  created_at: string;
  updated_at: string;
  messages: number;
};

export type SavedConversation = ConversationSummary & { items: SavedMessage[] };

type Hit = ConversationSummary & { snippet: string; matches: number };

/** Un nombre de archivo tranquilo a partir del título. */
function fileName(title: string) {
  const base = title
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^\w\s-]/g, "")
    .trim()
    .replace(/\s+/g, "-")
    .slice(0, 50)
    .toLowerCase();
  return `${base || "conversacion"}.md`;
}

export function ChatHistory({
  projectName,
  current,
  onOpen,
  onClose,
}: {
  projectName: (slug: string) => string;
  /** La conversación abierta ahora: se marca en la lista. */
  current: string;
  onOpen: (id: string) => void;
  onClose: () => void;
}) {
  const [items, setItems] = useState<ConversationSummary[] | null>(null);
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<Hit[] | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<ConversationSummary[]>("conversations_list")
      .then(setItems)
      .catch((e) => setError(String(e)));
  }, []);

  const search = async (e: React.FormEvent) => {
    e.preventDefault();
    setNotice(null);
    if (!query.trim()) {
      setHits(null);
      return;
    }
    try {
      setHits(await invoke<Hit[]>("conversations_search", { query, slug: null }));
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  };

  const exportOne = async (item: ConversationSummary) => {
    setNotice(null);
    try {
      const path = await save({ defaultPath: fileName(item.title), filters: [{ name: "Markdown", extensions: ["md"] }] });
      if (!path) return;
      await invoke("conversations_export", { id: item.id, path });
      setNotice(`Exportada a ${path}`);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  };

  const shown: (ConversationSummary | Hit)[] = hits ?? items ?? [];

  return (
    <section className="panel chat-historial">
      <header className="panel-head">
        <h2>Historial</h2>
        <div className="panel-aside">
          <button type="button" className="link" onClick={onClose}>
            Cerrar
          </button>
        </div>
      </header>
      <div className="panel-body">
        <form className="form-row" onSubmit={search}>
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Buscar en tus conversaciones: «SUPABASE_URL», «despliegue de Faro»…"
            aria-label="Buscar en el historial"
          />
          <button className="btn">Buscar</button>
          {hits && (
            <button type="button" className="btn" onClick={() => (setHits(null), setQuery(""))}>
              Ver todas
            </button>
          )}
        </form>
        {notice && <p className="stamp">{notice}</p>}
        {error && <p className="tablero-error">{error}</p>}
        {items && shown.length === 0 && (
          <p className="stamp">
            {hits ? `No aparece «${query}» en ninguna conversación.` : "Todavía no hay conversaciones guardadas."}
          </p>
        )}
        <ul className="chat-historial-list">
          {shown.map((c) => (
            <li key={c.id} className={c.id === current ? "on" : undefined}>
              <div>
                <button type="button" className="link" onClick={() => onOpen(c.id)} title="Abrirla y seguir desde ahí">
                  {c.title}
                </button>
                <span className="stamp">
                  {c.slug ? projectName(c.slug) : "Todos los proyectos"} · {c.model || "asistente"} · {ago(c.updated_at)} ·{" "}
                  {c.messages} mensajes
                  {"matches" in c && c.matches > 1 ? ` · ${c.matches} coincidencias` : ""}
                </span>
                {"snippet" in c && <p className="chat-historial-snippet">{c.snippet}</p>}
              </div>
              <button type="button" className="btn" onClick={() => exportOne(c)}>
                Exportar
              </button>
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
