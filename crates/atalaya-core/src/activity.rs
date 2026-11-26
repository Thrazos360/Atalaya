//! Actividad de las IA a las que Claude Code les encarga algo desde el chat (ver `delegate`):
//! quién empezó, qué va diciendo mientras trabaja y cuándo terminó, para que la vista Agente
//! muestre en vivo quién está haciendo qué.
//!
//! Como con las aprobaciones, la base hace de puente: el encargo corre en el servidor MCP, que
//! es otro proceso lanzado por Claude Code, y la app lee lo nuevo mientras la conversación sigue.

use crate::Registry;
use anyhow::Result;
use rusqlite::params;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Cuántas líneas se guardan de un mismo encargo: lo demás se resume al final.
const MAX_LINES: usize = 400;

pub(crate) fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_activity (
             id           INTEGER PRIMARY KEY AUTOINCREMENT,
             conversation TEXT NOT NULL,
             run          TEXT NOT NULL,
             actor        TEXT NOT NULL,
             kind         TEXT NOT NULL,
             text         TEXT NOT NULL,
             at           TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE INDEX IF NOT EXISTS chat_activity_conversation ON chat_activity (conversation, id);",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    /// Un encargo: sus líneas comparten este valor.
    pub run: String,
    /// La IA que lo hace.
    pub actor: String,
    /// `inicio`, `linea`, `fin` o `error`.
    pub kind: String,
    pub text: String,
    pub at: String,
}

pub fn log(registry: &Registry, conversation: &str, run: &str, actor: &str, kind: &str, text: &str) -> Result<()> {
    registry.conn().execute(
        "INSERT INTO chat_activity (conversation, run, actor, kind, text) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![conversation, run, actor, kind, text],
    )?;
    Ok(())
}

/// Lo que pasó en la conversación después de `after` (el último `id` que ya se vio).
pub fn since(registry: &Registry, conversation: &str, after: i64) -> Result<Vec<Entry>> {
    let mut stmt = registry.conn().prepare(
        "SELECT id, run, actor, kind, text, at FROM chat_activity WHERE conversation = ?1 AND id > ?2 ORDER BY id LIMIT 500",
    )?;
    let rows = stmt.query_map(params![conversation, after], |r| {
        Ok(Entry { id: r.get(0)?, run: r.get(1)?, actor: r.get(2)?, kind: r.get(3)?, text: r.get(4)?, at: r.get(5)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Una línea de la salida de un agente, legible: sin códigos de color, sin JSON crudo (se toma
/// su texto si lo trae) y corta. `None` si no dice nada.
pub fn readable(raw: &str) -> Option<String> {
    let mut line = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        // Secuencias de escape de la terminal (colores, cursor).
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\r' || (c.is_control() && c != '\t') {
            continue;
        }
        line.push(c);
    }
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if line.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let text = ["text", "message", "content", "result", "delta"]
            .iter()
            .find_map(|k| value[*k].as_str().or_else(|| value["item"][*k].as_str()).or_else(|| value["msg"][*k].as_str()))?;
        return readable(text);
    }
    let first = line.lines().next().unwrap_or(line);
    Some(if first.chars().count() > 300 { format!("{}…", first.chars().take(300).collect::<String>()) } else { first.to_string() })
}

/// Anota en vivo un encargo: `inicio` al crearlo, cada línea que el agente escribe mientras
/// trabaja (con `sink`, desde cualquier hilo) y `fin` o `error` al cerrarlo.
pub struct Recorder {
    sender: Option<Sender<(String, String)>>,
    writer: Option<JoinHandle<()>>,
}

impl Recorder {
    /// Sin conversación (el encargo no viene del chat) no anota nada.
    pub fn start(registry: &Registry, conversation: Option<&str>, actor: &str, what: &str) -> Self {
        let Some(conversation) = conversation.filter(|c| !c.is_empty()).map(String::from) else {
            return Self { sender: None, writer: None };
        };
        let path: PathBuf = registry.path();
        // La hora sola no basta: dos encargos de un reparto pueden empezar en el mismo
        // instante (el reloj de macOS va de a microsegundos) y se mezclarían en el chat.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let run = format!("{}-{now}-{n}", std::process::id());
        let actor = actor.to_string();
        let (sender, receiver) = channel::<(String, String)>();
        let _ = sender.send(("inicio".into(), what.to_string()));
        // Escribe desde un hilo propio con su conexión: el agente no espera a la base.
        let writer = std::thread::spawn(move || {
            let Ok(registry) = Registry::open(&path) else { return };
            let mut lines = 0usize;
            let mut skipped = 0usize;
            for (kind, text) in receiver {
                if kind == "linea" {
                    lines += 1;
                    if lines > MAX_LINES {
                        skipped += 1;
                        continue;
                    }
                }
                if kind != "linea" && skipped > 0 {
                    let _ = log(&registry, &conversation, &run, &actor, "linea", &format!("(… {skipped} líneas más)"));
                    skipped = 0;
                }
                let _ = log(&registry, &conversation, &run, &actor, &kind, &text);
                // Cerrado el encargo, lo que llegue después ya no es de él.
                if kind == "fin" || kind == "error" {
                    break;
                }
            }
        });
        Self { sender: Some(sender), writer: Some(writer) }
    }

    /// Para pasarle al agente: recibe cada línea de su salida.
    pub fn sink(&self) -> Option<Arc<dyn Fn(&str) + Send + Sync>> {
        let sender = Mutex::new(self.sender.clone()?);
        Some(Arc::new(move |raw: &str| {
            if let Some(text) = readable(raw) {
                let _ = sender.lock().unwrap().send(("linea".into(), text));
            }
        }))
    }

    /// Cierra el encargo y espera a que todo quede anotado.
    pub fn finish(mut self, ok: bool, text: &str) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send((if ok { "fin" } else { "error" }.into(), text.to_string()));
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // Si se suelta sin cerrar (un error a medio camino), igual queda cerrado.
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(("error".into(), "Se interrumpió.".into()));
        }
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_lines_drop_colors_and_raw_json() {
        assert_eq!(readable("\u{1b}[32mListo\u{1b}[0m  ").as_deref(), Some("Listo"));
        assert_eq!(readable("   \r"), None);
        assert_eq!(readable(r#"{"type":"item","item":{"text":"Leyendo src/main.rs"}}"#).as_deref(), Some("Leyendo src/main.rs"));
        assert_eq!(readable(r#"{"type":"ping"}"#), None);
    }

    #[test]
    fn a_recorder_leaves_start_lines_and_end_in_order() {
        let dir = std::env::temp_dir().join(format!("atalaya-actividad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let registry = Registry::open(&dir.join("a.db")).unwrap();

        let recorder = Recorder::start(&registry, Some("c1"), "Falso", "Revisa el login");
        let sink = recorder.sink().unwrap();
        let other = sink.clone();
        std::thread::spawn(move || other("leyendo archivos")).join().unwrap();
        sink("");
        sink("probando");
        recorder.finish(true, "Terminó");

        let entries = since(&registry, "c1", 0).unwrap();
        let kinds: Vec<&str> = entries.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["inicio", "linea", "linea", "fin"]);
        assert!(entries.iter().all(|e| e.actor == "Falso" && e.run == entries[0].run));
        assert_eq!(since(&registry, "c1", entries[1].id).unwrap().len(), 2, "solo lo nuevo");
        assert!(since(&registry, "otra", 0).unwrap().is_empty());

        // Sin conversación no anota nada.
        Recorder::start(&registry, None, "Falso", "x").finish(true, "y");
        assert_eq!(since(&registry, "c1", 0).unwrap().len(), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
