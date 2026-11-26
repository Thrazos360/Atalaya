//! La memoria de los proyectos: lo durable que sale de una conversación («este repo usa
//! pnpm», «la tabla pagos no se toca sin respaldo»). Una IA la propone (`proponer_memoria`
//! del MCP), el usuario la aprueba, la edita o la descarta, y lo aprobado llega a todas las IA
//! junto a las instrucciones (ver `skills::for_ai`). El usuario también puede escribirla.
//! Nada se borra: lo que se olvida queda descartado.

use crate::Registry;
use anyhow::{Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

/// Cuánto texto de memoria recibe una IA en cada encargo.
const BUDGET: usize = 6_000;

pub fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS memories (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            slug TEXT,
            text TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'propuesta',
            source TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            decided_at TEXT
        );",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Memory {
    pub id: i64,
    /// El proyecto; `None` vale para todos.
    pub slug: Option<String>,
    pub text: String,
    /// propuesta, aprobada o descartada.
    pub status: String,
    /// Quién la propuso: la IA, o «usuario».
    pub source: String,
    pub created_at: String,
    pub decided_at: Option<String>,
}

const COLUMNS: &str = "id, slug, text, status, source, created_at, decided_at";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Memory> {
    Ok(Memory {
        id: r.get(0)?,
        slug: r.get(1)?,
        text: r.get(2)?,
        status: r.get(3)?,
        source: r.get(4)?,
        created_at: r.get(5)?,
        decided_at: r.get(6)?,
    })
}

pub fn get(registry: &Registry, id: i64) -> Result<Option<Memory>> {
    Ok(registry.conn().query_row(&format!("SELECT {COLUMNS} FROM memories WHERE id = ?1"), [id], row).optional()?)
}

/// Las propuestas y las aprobadas (las descartadas no), las más nuevas primero.
pub fn list(registry: &Registry) -> Result<Vec<Memory>> {
    let mut stmt = registry
        .conn()
        .prepare(&format!("SELECT {COLUMNS} FROM memories WHERE status != 'descartada' ORDER BY id DESC"))?;
    Ok(stmt.query_map([], row)?.collect::<rusqlite::Result<Vec<_>>>()?)
}

fn clean(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() {
        bail!("la memoria está vacía");
    }
    if text.chars().count() > 600 {
        bail!("la memoria es muy larga: déjala en una o dos frases");
    }
    Ok(text.to_string())
}

fn known_project(registry: &Registry, slug: Option<&str>) -> Result<Option<String>> {
    match slug.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some(s) => match registry.get(s)? {
            Some(p) => Ok(Some(p.slug)),
            None => bail!("no hay un proyecto «{s}»"),
        },
    }
}

fn insert(registry: &Registry, slug: Option<&str>, text: &str, status: &str, source: &str) -> Result<Memory> {
    let slug = known_project(registry, slug)?;
    let text = clean(text)?;
    // Lo mismo dos veces no: si ya está (propuesta o aprobada), queda la que había.
    let same: Option<i64> = registry
        .conn()
        .query_row(
            "SELECT id FROM memories WHERE slug IS ?1 AND lower(text) = lower(?2) AND status != 'descartada'",
            params![slug, text],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = same {
        return Ok(get(registry, id)?.expect("existe"));
    }
    let decided = (status == "aprobada").then_some("now");
    registry.conn().execute(
        "INSERT INTO memories (slug, text, status, source, decided_at) VALUES (?1, ?2, ?3, ?4, CASE WHEN ?5 IS NULL THEN NULL ELSE datetime('now') END)",
        params![slug, text, status, source, decided],
    )?;
    Ok(get(registry, registry.conn().last_insert_rowid())?.expect("recién creada"))
}

/// Una IA propone algo para recordar: queda esperando al usuario.
pub fn propose(registry: &Registry, slug: Option<&str>, text: &str, source: &str) -> Result<Memory> {
    insert(registry, slug, text, "propuesta", source)
}

/// El usuario la escribe: queda aprobada.
pub fn add(registry: &Registry, slug: Option<&str>, text: &str) -> Result<Memory> {
    insert(registry, slug, text, "aprobada", "usuario")
}

/// Aprueba (con el texto ajustado, si viene) o descarta.
pub fn decide(registry: &Registry, id: i64, approve: bool, text: Option<&str>) -> Result<Memory> {
    let memory = get(registry, id)?.ok_or_else(|| anyhow::anyhow!("no hay una memoria #{id}"))?;
    let text = match text {
        Some(t) => clean(t)?,
        None => memory.text,
    };
    registry.conn().execute(
        "UPDATE memories SET status = ?2, text = ?3, decided_at = datetime('now') WHERE id = ?1",
        params![id, if approve { "aprobada" } else { "descartada" }, text],
    )?;
    Ok(get(registry, id)?.expect("existe"))
}

/// Cambia el texto de una memoria.
pub fn edit(registry: &Registry, id: i64, text: &str) -> Result<Memory> {
    let text = clean(text)?;
    if registry.conn().execute("UPDATE memories SET text = ?2 WHERE id = ?1", params![id, text])? == 0 {
        bail!("no hay una memoria #{id}");
    }
    Ok(get(registry, id)?.expect("existe"))
}

/// Lo que recibe una IA: lo aprobado para todos y lo de este proyecto.
pub fn for_ai(registry: &Registry, slug: Option<&str>) -> Result<String> {
    let mut stmt = registry.conn().prepare(
        "SELECT slug, text FROM memories WHERE status = 'aprobada' AND (slug IS NULL OR slug = ?1) ORDER BY slug IS NOT NULL, id",
    )?;
    let items = stmt
        .query_map([slug], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if items.is_empty() {
        return Ok(String::new());
    }
    let mut out = String::from("\n\nMemoria (lo aprendido en conversaciones anteriores, aprobado por el usuario):");
    for (of, text) in items {
        let line = match of {
            Some(s) => format!("\n- [{s}] {text}"),
            None => format!("\n- {text}"),
        };
        if out.len() + line.len() > BUDGET {
            out.push_str("\n- (hay más, sin espacio)");
            break;
        }
        out.push_str(&line);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposed_approved_and_delivered() {
        let dir = std::env::temp_dir().join(format!("atalaya-memoria-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Registry::open(&dir.join("a.db")).unwrap();
        r.upsert(&crate::Project {
            slug: "faro".into(),
            name: "Faro".into(),
            description: None,
            local_path: None,
            repo: None,
            production_url: None,
            notes: None,
            vercel_project_id: None,
            vercel_team_id: None,
            supabase_ref: None,
            updated_at: String::new(),
        })
        .unwrap();

        let m = propose(&r, Some("faro"), "Usa pnpm, no npm.", "claude-code").unwrap();
        assert_eq!(m.status, "propuesta");
        // Lo propuesto no llega a nadie todavía.
        assert_eq!(for_ai(&r, Some("faro")).unwrap(), "");
        // Repetida, queda la misma.
        assert_eq!(propose(&r, Some("faro"), "usa pnpm, no npm.", "codex").unwrap().id, m.id);

        decide(&r, m.id, true, Some("Usa pnpm (no npm) para instalar.")).unwrap();
        add(&r, None, "Responde corto.").unwrap();
        let other = propose(&r, Some("faro"), "Algo pasajero.", "claude-code").unwrap();
        decide(&r, other.id, false, None).unwrap();

        let faro = for_ai(&r, Some("faro")).unwrap();
        assert!(faro.contains("- Responde corto.") && faro.contains("[faro] Usa pnpm (no npm)"), "{faro}");
        assert!(!faro.contains("pasajero"));
        let elsewhere = for_ai(&r, None).unwrap();
        assert!(elsewhere.contains("Responde corto.") && !elsewhere.contains("pnpm"));
        assert_eq!(list(&r).unwrap().len(), 2);

        assert!(propose(&r, Some("otro"), "x", "ia").is_err());
        assert!(propose(&r, None, "   ", "ia").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
