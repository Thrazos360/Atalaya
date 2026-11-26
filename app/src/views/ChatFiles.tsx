import { type MutableRefObject, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";

/**
 * Los adjuntos del chat: pegar una captura, arrastrar archivos o elegirlos con 📎.
 * Atalaya los guarda en su carpeta y cada IA los recibe a su manera (ver `attachments.rs`).
 */

export type Attachment = {
  path: string;
  name: string;
  kind: "imagen" | "pdf" | "texto" | "archivo";
  mime: string;
  size: number;
  /** La imagen misma, para verla en el chat. */
  preview?: string | null;
};

const ICON: Record<Attachment["kind"], string> = { imagen: "▣", pdf: "▤", texto: "≡", archivo: "◇" };

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** El contenido de un archivo en base64, sin el prefijo `data:`. */
function base64(file: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ""));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

/** Una captura pegada llega como «image.png»: mejor un nombre que diga qué es y cuándo. */
function pastedName(file: File): string {
  if (file.name && file.name !== "image.png") return file.name;
  const ext = file.type.split("/")[1]?.replace("jpeg", "jpg") || "png";
  const now = new Date();
  const hh = [now.getHours(), now.getMinutes(), now.getSeconds()].map((n) => String(n).padStart(2, "0")).join("");
  return `captura-${hh}.${ext}`;
}

/** Los adjuntos que esperan en la caja, y las tres maneras de sumar uno. */
export function useAttachments(conversation: MutableRefObject<string>, visible: boolean) {
  const [files, setFiles] = useState<Attachment[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [dragging, setDragging] = useState(false);
  const [adding, setAdding] = useState(false);
  const visibleRef = useRef(visible);
  visibleRef.current = visible;

  const add = async (task: () => Promise<Attachment[]>) => {
    setError(null);
    setAdding(true);
    try {
      const added = await task();
      setFiles((all) => [...all, ...added]);
    } catch (e) {
      setError(String(e));
    } finally {
      setAdding(false);
    }
  };

  const addPaths = (paths: string[]) =>
    paths.length > 0 && add(() => invoke<Attachment[]>("chat_attach_paths", { conversation: conversation.current, paths }));

  /** Lo que pegaste: si trae archivos (una captura), se adjuntan y no se pega texto. */
  const onPaste = (e: React.ClipboardEvent) => {
    const pasted = Array.from(e.clipboardData.files);
    if (!pasted.length) return;
    e.preventDefault();
    add(() =>
      Promise.all(
        pasted.map(async (file) =>
          invoke<Attachment>("chat_attach_data", {
            conversation: conversation.current,
            name: pastedName(file),
            data: await base64(file),
          }),
        ),
      ),
    );
  };

  const pick = async () => {
    const chosen = await open({ multiple: true, title: "Adjuntar archivos" });
    if (!chosen) return;
    addPaths(Array.isArray(chosen) ? chosen : [chosen]);
  };

  // Arrastrar archivos a la ventana: Tauri entrega las rutas, sin pasar el contenido por la página.
  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (!visibleRef.current) return;
        const p = event.payload;
        if (p.type === "enter" || p.type === "over") setDragging(true);
        else if (p.type === "leave") setDragging(false);
        else if (p.type === "drop") {
          setDragging(false);
          addPaths(p.paths);
        }
      })
      .then((unlisten) => (gone ? unlisten() : (off = unlisten)))
      .catch(() => {});
    return () => {
      gone = true;
      off?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const remove = (path: string) => setFiles((all) => all.filter((f) => f.path !== path));
  const clear = () => setFiles([]);

  return { files, error, dragging, adding, onPaste, pick, remove, clear, dismissError: () => setError(null) };
}

/** Los adjuntos de un mensaje: miniaturas de las imágenes y una ficha para lo demás. */
export function FileStrip({ files, onRemove }: { files: Attachment[]; onRemove?: (path: string) => void }) {
  if (!files.length) return null;
  return (
    <div className="adjuntos">
      {files.map((f) => (
        <div key={f.path} className={`adjunto${f.preview ? " con-imagen" : ""}`} title={f.path}>
          {f.preview ? (
            <img src={f.preview} alt={f.name} />
          ) : (
            <span className="adjunto-icono" aria-hidden>
              {ICON[f.kind]}
            </span>
          )}
          <span className="adjunto-nombre">{f.name}</span>
          <span className="adjunto-peso">{size(f.size)}</span>
          {onRemove && (
            <button
              type="button"
              className="adjunto-quitar"
              onClick={() => onRemove(f.path)}
              aria-label={`No adjuntar ${f.name}`}
              title="No lo adjunta (el archivo queda guardado)"
            >
              ×
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
