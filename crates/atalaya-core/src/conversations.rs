//! El historial del chat: cada conversación del Agente queda guardada para buscarla,
//! retomarla o exportarla a Markdown. Solo se guarda el texto (lo que escribiste, lo que
//! respondió cada IA y los errores), no los pasos internos ni los adjuntos.

use crate::Registry;
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub(crate) fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS conversations (
             id         TEXT PRIMARY KEY,
             title      TEXT NOT NULL,
             slug       TEXT,
             model      TEXT NOT NULL DEFAULT '',
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE IF NOT EXISTS conversation_messages (
             conversation TEXT NOT NULL,
             idx          INTEGER NOT NULL,
             role         TEXT NOT NULL,
             author       TEXT NOT NULL DEFAULT '',
             text         TEXT NOT NULL,
             at           TEXT NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY (conversation, idx)
         );",
    )?;
    Ok(())
}

/// Un mensaje: `usuario`, `ia` o `nota` (errores y avisos de Atalaya).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: String,
    #[serde(default)]
    pub author: String,
    pub text: String,
    #[serde(default)]
    pub at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub slug: Option<String>,
    pub model: String,
    pub created_at: String,
    pub updated_at: String,
    pub messages: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    #[serde(flatten)]
    pub summary: Summary,
    pub items: Vec<Message>,
}

/// Una conversación que coincide con lo buscado, con un trozo alrededor de la primera coincidencia.
#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    #[serde(flatten)]
    pub summary: Summary,
    pub snippet: String,
    pub matches: i64,
}

const ROLES: [&str; 3] = ["usuario", "ia", "nota"];

fn clip(text: &str, max: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        format!("{}…", one.chars().take(max).collect::<String>().trim_end())
    }
}

/// Guarda la conversación tal como está. Se puede llamar después de cada turno: los
/// mensajes se guardan por posición, así que lo ya guardado se actualiza y lo nuevo se agrega.
pub fn save(registry: &Registry, id: &str, slug: Option<&str>, model: &str, messages: &[Message]) -> Result<()> {
    if id.trim().is_empty() {
        bail!("la conversación necesita un identificador");
    }
    if let Some(bad) = messages.iter().find(|m| !ROLES.contains(&m.role.as_str())) {
        bail!("rol de mensaje inválido «{}»", bad.role);
    }
    let Some(first) = messages.iter().find(|m| m.role == "usuario" && !m.text.trim().is_empty()) else {
        // Sin nada tuyo todavía no hay conversación que guardar.
        return Ok(());
    };
    let title = clip(&first.text, 80);
    let conn = registry.conn();
    conn.execute(
        "INSERT INTO conversations (id, title, slug, model) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET slug = excluded.slug, model = excluded.model, updated_at = datetime('now')",
        params![id, title, slug.filter(|s| !s.is_empty()), model],
    )?;
    for (idx, m) in messages.iter().enumerate() {
        conn.execute(
            "INSERT INTO conversation_messages (conversation, idx, role, author, text) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(conversation, idx) DO UPDATE SET role = excluded.role, author = excluded.author, text = excluded.text
             WHERE text != excluded.text OR author != excluded.author OR role != excluded.role",
            params![id, idx as i64, m.role, m.author, m.text],
        )?;
    }
    Ok(())
}

const SUMMARY: &str = "SELECT c.id, c.title, c.slug, c.model, c.created_at, c.updated_at,
        (SELECT count(*) FROM conversation_messages m WHERE m.conversation = c.id)
    FROM conversations c";

fn summary(r: &rusqlite::Row) -> rusqlite::Result<Summary> {
    Ok(Summary {
        id: r.get(0)?,
        title: r.get(1)?,
        slug: r.get(2)?,
        model: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
        messages: r.get(6)?,
    })
}

/// Las últimas conversaciones, la más reciente primero.
pub fn list(registry: &Registry, limit: usize) -> Result<Vec<Summary>> {
    let mut stmt = registry.conn().prepare(&format!("{SUMMARY} ORDER BY c.updated_at DESC, c.rowid DESC LIMIT ?1"))?;
    let rows = stmt.query_map([limit as i64], summary)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Encuentra una conversación por su identificador o por su comienzo (como los commits).
pub fn resolve(registry: &Registry, id: &str) -> Result<String> {
    let id = id.trim();
    if id.len() < 4 {
        bail!("escribe al menos 4 caracteres del identificador");
    }
    let mut stmt = registry.conn().prepare("SELECT id FROM conversations WHERE id = ?1 OR id LIKE ?1 || '%' LIMIT 2")?;
    let found: Vec<String> = stmt.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    match found.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no hay una conversación «{id}»"),
        _ => bail!("«{id}» calza con más de una conversación; escribe más caracteres"),
    }
}

pub fn get(registry: &Registry, id: &str) -> Result<Conversation> {
    let id = resolve(registry, id)?;
    let summary = registry
        .conn()
        .query_row(&format!("{SUMMARY} WHERE c.id = ?1"), [&id], summary)
        .optional()?
        .with_context(|| format!("no hay una conversación «{id}»"))?;
    let mut stmt = registry
        .conn()
        .prepare("SELECT role, author, text, at FROM conversation_messages WHERE conversation = ?1 ORDER BY idx")?;
    let items = stmt
        .query_map([&id], |r| Ok(Message { role: r.get(0)?, author: r.get(1)?, text: r.get(2)?, at: r.get(3)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Conversation { summary, items })
}

/// Un trozo del texto alrededor de `needle` (ya en minúsculas).
fn snippet(text: &str, needle: &str) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    let lower: Vec<char> = flat.to_lowercase().chars().collect();
    let target: Vec<char> = needle.chars().collect();
    // `to_lowercase` puede cambiar el largo de algunos caracteres raros; entonces se muestra el comienzo.
    let at = if lower.len() == chars.len() {
        lower.windows(target.len().max(1)).position(|w| w == target.as_slice()).unwrap_or(0)
    } else {
        0
    };
    let start = at.saturating_sub(60);
    let end = (at + target.len() + 100).min(chars.len());
    let mut piece: String = chars[start..end].iter().collect();
    if start > 0 {
        piece = format!("…{piece}");
    }
    if end < chars.len() {
        piece.push('…');
    }
    piece
}

/// Busca en el título y en todos los mensajes, sin distinguir mayúsculas. Con `slug`, solo
/// en las conversaciones de ese proyecto.
pub fn search(registry: &Registry, query: &str, slug: Option<&str>, limit: usize) -> Result<Vec<Hit>> {
    let needle = query.trim().to_lowercase();
    if needle.chars().count() < 2 {
        bail!("escribe al menos 2 letras para buscar");
    }
    // LIKE de SQLite solo ignora mayúsculas en ASCII: se filtra aquí, con todo Unicode.
    let mut stmt = registry.conn().prepare(&format!(
        "{SUMMARY} WHERE (?1 IS NULL OR c.slug = ?1) ORDER BY c.updated_at DESC, c.rowid DESC"
    ))?;
    let all: Vec<Summary> = stmt.query_map([slug.filter(|s| !s.is_empty())], summary)?.collect::<rusqlite::Result<_>>()?;
    let mut hits = Vec::new();
    for s in all {
        let mut texts = vec![s.title.clone()];
        let mut msgs = registry
            .conn()
            .prepare("SELECT text FROM conversation_messages WHERE conversation = ?1 ORDER BY idx")?;
        texts.extend(msgs.query_map([&s.id], |r| r.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?);
        let matching: Vec<&String> = texts.iter().filter(|t| t.to_lowercase().contains(&needle)).collect();
        // El título repite el primer mensaje: no cuenta dos veces.
        let matches = texts.iter().skip(1).filter(|t| t.to_lowercase().contains(&needle)).count() as i64;
        if let Some(first) = matching.iter().find(|t| **t != &s.title).or(matching.first()) {
            hits.push(Hit { snippet: snippet(first, &needle), matches: matches.max(1), summary: s });
            if hits.len() >= limit {
                break;
            }
        }
    }
    Ok(hits)
}

fn label(m: &Message) -> String {
    match m.role.as_str() {
        "usuario" => "Tú".into(),
        "nota" => "Atalaya".into(),
        _ if m.author.is_empty() => "IA".into(),
        _ => m.author.clone(),
    }
}

/// La conversación en Markdown, lista para guardar o compartir.
pub fn markdown(registry: &Registry, id: &str) -> Result<String> {
    let c = get(registry, id)?;
    let project = match c.summary.slug.as_deref() {
        Some(slug) => registry.get(slug)?.map(|p| p.name).unwrap_or_else(|| slug.to_string()),
        None => "Todos los proyectos".into(),
    };
    let mut out = format!(
        "# {}\n\n- Proyecto: {project}\n- Con: {}\n- Empezó: {} (UTC)\n- Última vez: {} (UTC)\n- Exportada desde Atalaya\n",
        c.summary.title,
        if c.summary.model.is_empty() { "el asistente" } else { &c.summary.model },
        c.summary.created_at,
        c.summary.updated_at,
    );
    for m in &c.items {
        out.push_str(&format!("\n---\n\n**{}**", label(m)));
        if !m.at.is_empty() {
            out.push_str(&format!(" · {}", m.at.get(11..16).unwrap_or(&m.at)));
        }
        out.push_str(&format!("\n\n{}\n", m.text.trim()));
    }
    Ok(out)
}

/// Escribe el Markdown en `path`. La ruta la elige el usuario (en la app, con el diálogo de
/// guardar de macOS, que ya pregunta antes de reemplazar).
pub fn export(registry: &Registry, id: &str, path: &Path) -> Result<()> {
    let text = markdown(registry, id)?;
    std::fs::write(path, text).with_context(|| format!("no se pudo escribir {}", path.display()))
}

/// Lo conversado, para retomarlo con una IA que no lo vio: lo último primero si no cabe.
pub fn recap(registry: &Registry, id: &str, max_chars: usize) -> Result<String> {
    let c = get(registry, id)?;
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0;
    for m in c.items.iter().rev() {
        let line = format!("{}: {}", label(m), m.text.trim());
        used += line.chars().count();
        if used > max_chars && !lines.is_empty() {
            lines.push("(lo anterior quedó fuera por largo)".into());
            break;
        }
        lines.push(line);
    }
    lines.reverse();
    Ok(lines.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, author: &str, text: &str) -> Message {
        Message { role: role.into(), author: author.into(), text: text.into(), at: String::new() }
    }

    #[test]
    fn a_conversation_is_saved_turn_by_turn_and_found_again() {
        let r = crate::testutil::registry("conversaciones");
        crate::testutil::project(&r, "faro");
        // Sin nada tuyo no se guarda.
        save(&r, "abcd-1", None, "Claude Code", &[msg("nota", "", "Error al conectar")]).unwrap();
        assert!(list(&r, 10).unwrap().is_empty());

        let mut turn = vec![msg("usuario", "", "¿Por qué falló el despliegue de Faro?"), msg("ia", "Claude Code", "Faltaba la variable SUPABASE_URL.")];
        save(&r, "abcd-1", Some("faro"), "Claude Code", &turn).unwrap();
        turn.push(msg("usuario", "", "Agrégala en Vercel"));
        turn.push(msg("ia", "Claude Code", "Listo, quedó en Producción."));
        save(&r, "abcd-1", Some("faro"), "Claude Code", &turn).unwrap();
        save(&r, "efgh-2", None, "DeepSeek", &[msg("usuario", "", "Resume la semana"), msg("ia", "DeepSeek", "Nada grave.")])
            .unwrap();

        let all = list(&r, 10).unwrap();
        assert_eq!(all.len(), 2);
        let faro = get(&r, "abcd").unwrap();
        assert_eq!(faro.summary.title, "¿Por qué falló el despliegue de Faro?");
        assert_eq!(faro.items.len(), 4, "el segundo guardado agrega, no duplica");

        // Sin distinguir mayúsculas ni tildes de por medio, y con el trozo alrededor.
        let hits = search(&r, "supabase_url", None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("SUPABASE_URL"));
        assert_eq!(search(&r, "FALLÓ", None, 10).unwrap().len(), 1);
        assert_eq!(search(&r, "semana", Some("faro"), 10).unwrap().len(), 0, "filtra por proyecto");
        assert!(search(&r, "a", None, 10).is_err());

        let md = markdown(&r, "abcd-1").unwrap();
        assert!(md.starts_with("# ¿Por qué falló el despliegue de Faro?"));
        assert!(md.contains("- Proyecto: faro") && md.contains("**Tú**") && md.contains("**Claude Code**"));
        assert!(md.contains("Listo, quedó en Producción."));

        let recap = recap(&r, "abcd-1", 60).unwrap();
        assert!(recap.ends_with("Claude Code: Listo, quedó en Producción."));
        assert!(recap.contains("quedó fuera"));

        assert!(resolve(&r, "zzzz").is_err());
        assert!(save(&r, "x", None, "", &[msg("sistema", "", "hola")]).is_err());
    }

    #[test]
    fn export_writes_markdown() {
        let r = crate::testutil::registry("exportar");
        save(&r, "1234-5678", None, "", &[msg("usuario", "", "Hola"), msg("ia", "", "¿Qué necesitas?")]).unwrap();
        let path = std::env::temp_dir().join(format!("atalaya-export-{}.md", std::process::id()));
        export(&r, "1234", &path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("**IA**") && text.contains("¿Qué necesitas?") && text.contains("el asistente"));
        std::fs::remove_file(path).unwrap();
    }
}
