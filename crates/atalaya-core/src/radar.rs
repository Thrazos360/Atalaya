//! Radar: vigila páginas de la competencia y avisa cuando cambian.
//!
//! Cada día baja el texto de cada página registrada y lo compara con la versión
//! anterior. Si cambió, guarda qué líneas aparecieron y cuáles desaparecieron, y le
//! pide a Claude un resumen de lo que importa (precios, planes, funciones).

use crate::Registry;
use anyhow::{Context, Result, bail};
use rusqlite::params;
use serde::Serialize;
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct Competitor {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub url: String,
    pub last_checked: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Event {
    pub id: i64,
    pub slug: String,
    pub competitor: String,
    pub url: String,
    pub summary: String,
    pub detail: String,
    pub at: String,
}

pub(crate) fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS competitors (
             id           INTEGER PRIMARY KEY,
             slug         TEXT NOT NULL,
             name         TEXT NOT NULL,
             url          TEXT NOT NULL,
             last_text    TEXT,
             last_checked TEXT,
             last_error   TEXT
         );
         CREATE TABLE IF NOT EXISTS radar_events (
             id            INTEGER PRIMARY KEY,
             competitor_id INTEGER NOT NULL,
             summary       TEXT NOT NULL,
             detail        TEXT NOT NULL,
             at            TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )?;
    Ok(())
}

pub fn add(registry: &Registry, slug: &str, name: &str, url: &str) -> Result<i64> {
    registry.get(slug)?.with_context(|| format!("no existe el proyecto '{slug}'"))?;
    if !url.starts_with("https://") && !url.starts_with("http://") {
        bail!("la dirección debe empezar con https://");
    }
    if name.trim().is_empty() {
        bail!("falta el nombre del competidor");
    }
    registry.conn().execute(
        "INSERT INTO competitors (slug, name, url) VALUES (?1, ?2, ?3)",
        [slug, name.trim(), url.trim()],
    )?;
    Ok(registry.conn().last_insert_rowid())
}

pub fn remove(registry: &Registry, id: i64) -> Result<()> {
    registry.conn().execute("DELETE FROM radar_events WHERE competitor_id = ?1", [id])?;
    registry.conn().execute("DELETE FROM competitors WHERE id = ?1", [id])?;
    Ok(())
}

pub fn competitors(registry: &Registry) -> Result<Vec<Competitor>> {
    let mut stmt = registry.conn().prepare(
        "SELECT id, slug, name, url, last_checked, last_error FROM competitors ORDER BY slug, name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Competitor {
            id: r.get(0)?,
            slug: r.get(1)?,
            name: r.get(2)?,
            url: r.get(3)?,
            last_checked: r.get(4)?,
            last_error: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn events(registry: &Registry) -> Result<Vec<Event>> {
    let mut stmt = registry.conn().prepare(
        "SELECT e.id, c.slug, c.name, c.url, e.summary, e.detail, e.at
         FROM radar_events e JOIN competitors c ON c.id = e.competitor_id
         ORDER BY e.id DESC LIMIT 100",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Event {
            id: r.get(0)?,
            slug: r.get(1)?,
            competitor: r.get(2)?,
            url: r.get(3)?,
            summary: r.get(4)?,
            detail: r.get(5)?,
            at: r.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// El texto visible de una página: sin etiquetas, guiones ni estilos, una frase por línea.
pub fn page_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len() / 4);
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        text.push_str(&rest[..start]);
        let tag = &rest[start..];
        let lower = tag.get(..8).unwrap_or(tag).to_ascii_lowercase();
        // El contenido de <script> y <style> no es texto de la página.
        let skip_to = if lower.starts_with("<script") {
            Some("</script>")
        } else if lower.starts_with("<style") {
            Some("</style>")
        } else {
            None
        };
        let end = match skip_to {
            Some(close) => tag.to_ascii_lowercase().find(close).map(|i| i + close.len()),
            None => tag.find('>').map(|i| i + 1),
        };
        text.push('\n');
        match end {
            Some(end) => rest = &tag[end..],
            None => {
                rest = "";
                break;
            }
        }
    }
    text.push_str(rest);
    let decoded = text
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    let mut seen = HashSet::new();
    decoded
        .lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| line.chars().count() > 2 && seen.insert(line.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn fetch(url: &str) -> Result<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .user_agent("Mozilla/5.0 (Macintosh) Atalaya")
        .build()
        .into();
    let mut response = agent.get(url).call().context("no se pudo abrir la página")?;
    let code = response.status().as_u16();
    if code >= 400 {
        bail!("la página respondió con error {code}");
    }
    let html = response
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .context("no se pudo leer la página")?;
    let text = page_text(&html);
    if text.len() < 80 {
        bail!("la página casi no trae texto; puede que se arme con JavaScript y Atalaya no la pueda leer");
    }
    Ok(text)
}

/// Líneas que aparecieron y que desaparecieron entre dos versiones del texto.
fn changes(before: &str, after: &str) -> (Vec<String>, Vec<String>) {
    let old: HashSet<&str> = before.lines().collect();
    let new: HashSet<&str> = after.lines().collect();
    let added = after.lines().filter(|l| !old.contains(l)).map(String::from).collect();
    let removed = before.lines().filter(|l| !new.contains(l)).map(String::from).collect();
    (added, removed)
}

/// Le pide a Claude un resumen del cambio. Si no está disponible, devuelve `None`.
fn summarize(registry: &Registry, name: &str, detail: &str) -> Option<String> {
    let claude = crate::worker::find_claude()?;
    let prompt = format!(
        "La página de «{name}», un competidor, cambió. Abajo están las líneas que aparecieron (+) y las que \
desaparecieron (-). Resume en una o dos frases en español, en texto simple, qué cambió y si importa \
(precios, planes, funciones, mensajes). Si son cambios sin importancia, como fechas o textos menores, dilo \
en una frase. No inventes nada que no esté en las líneas.\n\n{}",
        detail.chars().take(6000).collect::<String>()
    );
    // Con JSON trae sus tokens, para anotarlo en el consumo.
    crate::ai::tracked_claude(registry, "radar", None, |last| {
        let out = Command::new(claude)
            .args(["-p", &prompt, "--output-format", "json", "--tools", "", "--strict-mcp-config", "--no-session-persistence"])
            .args(crate::ai::claude_cap_args(registry)?)
            .env("PATH", crate::worker::developer_path())
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()?;
        let reply: serde_json::Value = serde_json::from_slice(&out.stdout).context("Claude Code no respondió")?;
        let text = reply["result"].as_str().unwrap_or_default().trim().to_string();
        let failed = reply["is_error"].as_bool().unwrap_or(false);
        *last = Some(reply);
        if let Some(note) = last.as_ref().and_then(crate::ai::claude_cap_note) {
            anyhow::bail!("Claude Code no pudo resumir el cambio: {note}");
        }
        if failed || text.is_empty() {
            anyhow::bail!("Claude Code no pudo resumir el cambio");
        }
        Ok(text)
    })
    .ok()
}

/// Revisa un competidor. Devuelve `true` si registró un cambio.
pub fn check(registry: &Registry, id: i64) -> Result<bool> {
    let (name, url, previous): (String, String, Option<String>) = registry.conn().query_row(
        "SELECT name, url, last_text FROM competitors WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let text = match fetch(&url) {
        Ok(text) => text,
        Err(e) => {
            registry.conn().execute(
                "UPDATE competitors SET last_checked = datetime('now'), last_error = ?2 WHERE id = ?1",
                params![id, format!("{e:#}")],
            )?;
            return Err(e);
        }
    };
    registry.conn().execute(
        "UPDATE competitors SET last_text = ?2, last_checked = datetime('now'), last_error = NULL WHERE id = ?1",
        params![id, text],
    )?;
    // La primera lectura solo deja la referencia para comparar después.
    let Some(previous) = previous else { return Ok(false) };
    let (added, removed) = changes(&previous, &text);
    if added.is_empty() && removed.is_empty() {
        return Ok(false);
    }
    let detail = added
        .iter()
        .take(40)
        .map(|l| format!("+ {l}"))
        .chain(removed.iter().take(40).map(|l| format!("- {l}")))
        .collect::<Vec<_>>()
        .join("\n");
    let summary = summarize(registry, &name, &detail).unwrap_or_else(|| {
        format!("La página cambió: {} líneas nuevas y {} eliminadas.", added.len(), removed.len())
    });
    registry.conn().execute(
        "INSERT INTO radar_events (competitor_id, summary, detail) VALUES (?1, ?2, ?3)",
        params![id, summary, detail],
    )?;
    crate::checks::notify(registry, "radar", &format!("Radar: {name} cambió su página"), &summary);
    Ok(true)
}

/// Ronda del vigía: cada competidor una vez al día. Devuelve cuántos cambios encontró.
pub fn round(registry: &Registry) -> Result<usize> {
    let due: Vec<i64> = {
        let mut stmt = registry.conn().prepare(
            "SELECT id FROM competitors
             WHERE last_checked IS NULL OR (julianday('now') - julianday(last_checked)) >= 1",
        )?;
        stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    Ok(due.into_iter().filter(|id| check(registry, *id).unwrap_or(false)).count())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_visible_text_and_finds_changes() {
        let html = "<html><head><style>p{color:red}</style><script>var precio = 1;</script></head>\
<body><h1>Plan  Pro</h1><p>US$ 9,99 al mes</p><p>Soporte &amp; respaldo</p></body></html>";
        let text = page_text(html);
        assert_eq!(text, "Plan Pro\nUS$ 9,99 al mes\nSoporte & respaldo");
        let (added, removed) = changes(&text, &text.replace("9,99", "6,99"));
        assert_eq!(added, vec!["US$ 6,99 al mes"]);
        assert_eq!(removed, vec!["US$ 9,99 al mes"]);
    }
}
