//! Aprobaciones del chat: cuando el agente de la vista Agente quiere hacer algo que
//! cambia cosas (editar un archivo, ejecutar una orden, prender un interruptor), Claude
//! Code consulta la herramienta MCP `aprobar_accion`. Esa herramienta deja la pregunta
//! aquí y espera; la app la muestra en la conversación y guarda la respuesta.
//!
//! La base hace de puente porque el servidor MCP es otro proceso, lanzado por Claude Code.

use crate::Registry;
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub(crate) fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS approvals (
             id           INTEGER PRIMARY KEY AUTOINCREMENT,
             conversation TEXT NOT NULL,
             tool         TEXT NOT NULL,
             input        TEXT NOT NULL,
             status       TEXT NOT NULL DEFAULT 'pendiente',
             remember     INTEGER NOT NULL DEFAULT 0,
             message      TEXT,
             created_at   TEXT NOT NULL DEFAULT (datetime('now')),
             answered_at  TEXT
         );
         CREATE INDEX IF NOT EXISTS approvals_conversation ON approvals (conversation, status);",
    )?;
    // El paso de Claude Code que pide permiso (0.28), para mostrar quién lo pide: él o un subagente.
    let exists: bool = registry
        .conn()
        .query_row("SELECT count(*) > 0 FROM pragma_table_info('approvals') WHERE name = 'step'", [], |r| r.get(0))?;
    if !exists {
        registry.conn().execute_batch("ALTER TABLE approvals ADD COLUMN step TEXT")?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct Approval {
    pub id: i64,
    pub conversation: String,
    pub tool: String,
    /// Lo que se va a hacer, en una línea legible.
    pub summary: String,
    /// El detalle: la orden completa, el cambio en el archivo…
    pub detail: String,
    pub created_at: String,
    /// El paso de Claude Code que lo pide (`tool_use_id`), si se sabe.
    pub step: Option<String>,
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!("{}…", text.chars().take(max).collect::<String>())
}

/// Describe una acción para que una persona decida sin leer JSON.
pub fn describe(tool: &str, input: &Value) -> (String, String) {
    let s = |k: &str| input[k].as_str().unwrap_or_default().to_string();
    match tool {
        "Bash" => {
            let what = s("description");
            let summary = if what.is_empty() { "Ejecutar una orden".into() } else { what };
            (summary, s("command"))
        }
        "Edit" => (
            format!("Editar {}", s("file_path")),
            format!("Quita:\n{}\n\nPone:\n{}", clip(&s("old_string"), 1500), clip(&s("new_string"), 1500)),
        ),
        "MultiEdit" => (format!("Editar {}", s("file_path")), clip(&input["edits"].to_string(), 3000)),
        "Write" => (format!("Escribir {}", s("file_path")), clip(&s("content"), 3000)),
        "NotebookEdit" => (format!("Editar el cuaderno {}", s("notebook_path")), clip(&s("new_source"), 3000)),
        "WebFetch" => (format!("Leer {}", s("url")), s("prompt")),
        "mcp__atalaya__delegar_trabajo" => {
            let who = s("ia");
            (
                format!(
                    "Encargar un cambio en {} a {}",
                    s("proyecto"),
                    if who.is_empty() { "el constructor de tus roles".to_string() } else { who }
                ),
                clip(&s("tarea"), 3000),
            )
        }
        "mcp__atalaya__resolve_decision" => {
            let verb = if s("accion") == "descartar" { "Descartar" } else { "Aprobar" };
            (format!("{verb} la decisión #{}", input["id"]), s("texto"))
        }
        "mcp__atalaya__delegar_trabajos" => {
            let orders = input["encargos"].as_array().cloned().unwrap_or_default();
            let lines: Vec<String> = orders
                .iter()
                .map(|o| {
                    let who = o["ia"].as_str().filter(|w| !w.is_empty()).unwrap_or("el constructor que toque");
                    format!(
                        "• {} ({}): {}",
                        o["proyecto"].as_str().unwrap_or("?"),
                        who,
                        clip(o["tarea"].as_str().unwrap_or_default(), 600)
                    )
                })
                .collect();
            (format!("Repartir {} cambios a la vez entre tus agentes", orders.len()), clip(&lines.join("\n\n"), 3000))
        }
        other => {
            let name = other.strip_prefix("mcp__atalaya__").map(|n| format!("Atalaya: {n}")).unwrap_or_else(|| other.to_string());
            (name, clip(&serde_json::to_string_pretty(input).unwrap_or_default(), 3000))
        }
    }
}

/// Deja una pregunta. Si en esta conversación ya se permitió siempre esa herramienta,
/// queda permitida al tiro.
pub fn ask(registry: &Registry, conversation: &str, tool: &str, input: &Value) -> Result<(i64, bool)> {
    let remembered: bool = registry.conn().query_row(
        "SELECT count(*) > 0 FROM approvals WHERE conversation = ?1 AND tool = ?2 AND status = 'permitida' AND remember = 1",
        params![conversation, tool],
        |r| r.get(0),
    )?;
    let status = if remembered { "permitida" } else { "pendiente" };
    registry.conn().execute(
        "INSERT INTO approvals (conversation, tool, input, status, answered_at)
         VALUES (?1, ?2, ?3, ?4, CASE WHEN ?4 = 'pendiente' THEN NULL ELSE datetime('now') END)",
        params![conversation, tool, input.to_string(), status],
    )?;
    Ok((registry.conn().last_insert_rowid(), remembered))
}

/// La respuesta, cuando ya la hay: (permitida, mensaje).
pub fn status(registry: &Registry, id: i64) -> Result<Option<(bool, Option<String>)>> {
    let row: Option<(String, Option<String>)> = registry
        .conn()
        .query_row("SELECT status, message FROM approvals WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(match row {
        Some((s, message)) if s != "pendiente" => Some((s == "permitida", message)),
        Some(_) => None,
        None => Some((false, Some("la pregunta ya no existe".into()))),
    })
}

/// Espera la respuesta. Sin respuesta en `timeout`, se rechaza.
pub fn wait(registry: &Registry, id: i64, timeout: Duration) -> Result<(bool, Option<String>)> {
    let start = Instant::now();
    loop {
        if let Some(answer) = status(registry, id)? {
            return Ok(answer);
        }
        if start.elapsed() > timeout {
            answer(registry, id, false, false, Some("nadie respondió a tiempo"))?;
            return Ok((false, Some("nadie respondió a tiempo".into())));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// La respuesta que espera Claude Code de su herramienta de permisos.
pub fn decision_json(allowed: bool, message: Option<&str>, input: &Value) -> String {
    if allowed {
        // Las entradas muy grandes no se devuelven: sin `updatedInput` se usa la original.
        if input.to_string().len() < 30_000 {
            json!({ "behavior": "allow", "updatedInput": input }).to_string()
        } else {
            json!({ "behavior": "allow" }).to_string()
        }
    } else {
        json!({ "behavior": "deny", "message": message.unwrap_or("El usuario no lo permitió.") }).to_string()
    }
}

/// Todo el trámite, para la herramienta MCP: preguntar, esperar y responder a Claude Code.
pub fn request(registry: &Registry, conversation: &str, tool: &str, input: &Value, timeout: Duration) -> Result<String> {
    request_step(registry, conversation, tool, input, None, timeout)
}

/// Como `request`, anotando qué paso de Claude Code lo pide.
pub fn request_step(
    registry: &Registry,
    conversation: &str,
    tool: &str,
    input: &Value,
    step: Option<&str>,
    timeout: Duration,
) -> Result<String> {
    let (id, remembered) = ask(registry, conversation, tool, input)?;
    if let Some(step) = step {
        registry.conn().execute("UPDATE approvals SET step = ?2 WHERE id = ?1", params![id, step])?;
    }
    if remembered {
        return Ok(decision_json(true, None, input));
    }
    let (allowed, message) = wait(registry, id, timeout)?;
    Ok(decision_json(allowed, message.as_deref(), input))
}

pub fn pending(registry: &Registry, conversation: &str) -> Result<Vec<Approval>> {
    let mut stmt = registry.conn().prepare(
        "SELECT id, conversation, tool, input, created_at, step FROM approvals
         WHERE conversation = ?1 AND status = 'pendiente' ORDER BY id",
    )?;
    let rows = stmt.query_map([conversation], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
        ))
    })?;
    let mut list = Vec::new();
    for row in rows {
        let (id, conversation, tool, input, created_at, step) = row?;
        let input: Value = serde_json::from_str(&input).unwrap_or_default();
        let (summary, detail) = describe(&tool, &input);
        list.push(Approval { id, conversation, tool, summary, detail, created_at, step });
    }
    Ok(list)
}

/// Responde una pregunta. `remember` permite esa herramienta el resto de la conversación.
pub fn answer(registry: &Registry, id: i64, allow: bool, remember: bool, message: Option<&str>) -> Result<()> {
    let changed = registry.conn().execute(
        "UPDATE approvals SET status = ?2, remember = ?3, message = ?4, answered_at = datetime('now')
         WHERE id = ?1 AND status = 'pendiente'",
        params![id, if allow { "permitida" } else { "rechazada" }, allow && remember, message],
    )?;
    if changed == 0 {
        bail!("esa pregunta ya se respondió");
    }
    Ok(())
}

/// Al detener una conversación, lo que quedaba pendiente se rechaza.
pub fn cancel(registry: &Registry, conversation: &str) -> Result<usize> {
    Ok(registry.conn().execute(
        "UPDATE approvals SET status = 'rechazada', message = 'La conversación se detuvo.', answered_at = datetime('now')
         WHERE conversation = ?1 AND status = 'pendiente'",
        [conversation],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    #[test]
    fn asks_waits_and_remembers_per_conversation() {
        let r = testutil::registry("aprobaciones");
        let input = json!({ "command": "npm test", "description": "Corre las pruebas" });

        // Otra hebra responde como lo haría la app.
        let path = r.path();
        let answerer = std::thread::spawn(move || {
            let r = Registry::open(&path).unwrap();
            loop {
                if let Some(p) = pending(&r, "c1").unwrap().first() {
                    assert_eq!(p.summary, "Corre las pruebas");
                    assert_eq!(p.detail, "npm test");
                    assert_eq!(p.step.as_deref(), Some("toolu_1"), "sabe qué paso lo pide");
                    answer(&r, p.id, true, true, None).unwrap();
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let reply: Value = serde_json::from_str(&request_step(&r, "c1", "Bash", &input, Some("toolu_1"), Duration::from_secs(10)).unwrap()).unwrap();
        answerer.join().unwrap();
        assert_eq!(reply["behavior"], "allow");
        assert_eq!(reply["updatedInput"], input);

        // Permitida siempre: la segunda vez no pregunta. En otra conversación, sí.
        let (_, remembered) = ask(&r, "c1", "Bash", &input).unwrap();
        assert!(remembered);
        let (id, remembered) = ask(&r, "c2", "Bash", &input).unwrap();
        assert!(!remembered);

        // Rechazar, o quedarse sin respuesta, le dice a Claude por qué.
        answer(&r, id, false, false, Some("no toques eso")).unwrap();
        assert!(answer(&r, id, true, false, None).is_err(), "no se responde dos veces");
        let reply: Value = serde_json::from_str(&request(&r, "c3", "Write", &json!({}), Duration::from_millis(400)).unwrap()).unwrap();
        assert_eq!(reply["behavior"], "deny");
        assert_eq!(reply["message"], "nadie respondió a tiempo");

        // Detener la conversación rechaza lo pendiente.
        ask(&r, "c4", "Edit", &json!({ "file_path": "a.rs" })).unwrap();
        assert_eq!(cancel(&r, "c4").unwrap(), 1);
        assert!(pending(&r, "c4").unwrap().is_empty());
    }

    #[test]
    fn describes_actions_for_people() {
        let (s, d) = describe("Edit", &json!({ "file_path": "/p/a.rs", "old_string": "x", "new_string": "y" }));
        assert_eq!(s, "Editar /p/a.rs");
        assert!(d.contains("Quita:\nx") && d.contains("Pone:\ny"));
        assert_eq!(describe("mcp__atalaya__set_flag", &json!({ "on": true })).0, "Atalaya: set_flag");
        let (s, d) = describe(
            "mcp__atalaya__delegar_trabajos",
            &json!({ "encargos": [
                { "tarea": "arregla el login", "proyecto": "web", "ia": "codex" },
                { "tarea": "sube la versión", "proyecto": "api" }
            ] }),
        );
        assert_eq!(s, "Repartir 2 cambios a la vez entre tus agentes");
        assert_eq!(describe("mcp__atalaya__resolve_decision", &json!({ "id": 4, "accion": "aprobar" })).0, "Aprobar la decisión #4");
        assert!(d.contains("• web (codex): arregla el login") && d.contains("• api (el constructor que toque)"), "{d}");
    }
}
