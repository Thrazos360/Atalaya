//! Registro de proyectos: lo poco que no se puede deducir del repo.
//! Todo lo demás (stack, commits, servicios) se lee en vivo en `inspect`.

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub local_path: Option<String>,
    /// `dueño/repo` en GitHub.
    pub repo: Option<String>,
    pub production_url: Option<String>,
    pub notes: Option<String>,
    /// Proyecto de Vercel, cuando la carpeta local no está enlazada con `vercel link`.
    #[serde(default)]
    pub vercel_project_id: Option<String>,
    #[serde(default)]
    pub vercel_team_id: Option<String>,
    /// Referencia del proyecto de Supabase, cuando no se puede deducir de la carpeta.
    #[serde(default)]
    pub supabase_ref: Option<String>,
    #[serde(default)]
    pub updated_at: String,
}

pub struct Registry {
    conn: Connection,
}

/// "Mi Proyecto 2" → "mi-proyecto-2".
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

const COLUMNS: &str =
    "slug, name, description, local_path, repo, production_url, notes, updated_at, \
     vercel_project_id, vercel_team_id, supabase_ref";

impl Registry {
    /// `ATALAYA_DB` si está definida; si no, la carpeta de datos del sistema
    /// (`~/Library/Application Support/Atalaya` en macOS, `~/.local/share/Atalaya` en Linux).
    pub fn default_path() -> Result<PathBuf> {
        if let Some(p) = std::env::var_os("ATALAYA_DB") {
            return Ok(PathBuf::from(p));
        }
        let dir = dirs::data_dir().context("no se encontró la carpeta de datos del sistema")?;
        Ok(dir.join("Atalaya").join("atalaya.db"))
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&Self::default_path()?)
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("no se pudo abrir {}", path.display()))?;
        // La app, el vigía y el servidor MCP escriben a la vez: se espera en vez de fallar.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS projects (
                 slug           TEXT PRIMARY KEY,
                 name           TEXT NOT NULL,
                 description    TEXT,
                 local_path     TEXT,
                 repo           TEXT,
                 production_url TEXT,
                 notes          TEXT,
                 updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )?;
        // Columnas añadidas después de la primera versión.
        for column in ["vercel_project_id", "vercel_team_id", "supabase_ref"] {
            let exists: bool = conn.query_row(
                "SELECT count(*) > 0 FROM pragma_table_info('projects') WHERE name = ?1",
                [column],
                |r| r.get(0),
            )?;
            if !exists {
                conn.execute_batch(&format!("ALTER TABLE projects ADD COLUMN {column} TEXT"))?;
            }
        }
        let registry = Self { conn };
        registry.init_checks()?;
        crate::decisions::init(&registry)?;
        crate::radar::init(&registry)?;
        crate::connections::init(&registry)?;
        crate::ai::init(&registry)?;
        crate::approvals::init(&registry)?;
        crate::activity::init(&registry)?;
        crate::skills::init(&registry)?;
        crate::mcp_servers::init(&registry)?;
        crate::memories::init(&registry)?;
        crate::commands::init(&registry)?;
        crate::automations::init(&registry)?;
        crate::telegram::init(&registry)?;
        crate::conversations::init(&registry)?;
        // Columnas del flujo de trabajo de las mejoras: rama, PR e informe (0.8) y la
        // conversación con el usuario cuando el agente pregunta algo (0.26).
        for column in ["branch", "pr_url", "result", "feedback"] {
            let exists: bool = registry.conn.query_row(
                "SELECT count(*) > 0 FROM pragma_table_info('improvements') WHERE name = ?1",
                [column],
                |r| r.get(0),
            )?;
            if !exists {
                registry
                    .conn
                    .execute_batch(&format!("ALTER TABLE improvements ADD COLUMN {column} TEXT"))?;
            }
        }
        Ok(registry)
    }

    /// Conexión a la base, para los módulos del núcleo que guardan sus propias tablas.
    /// El archivo de la base.
    pub fn path(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(self.conn.path().unwrap_or_default())
    }

    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Crea el proyecto o reemplaza sus datos si el slug ya existe.
    pub fn upsert(&self, p: &Project) -> Result<()> {
        self.conn.execute(
            "INSERT INTO projects (slug, name, description, local_path, repo, production_url, notes,
                                   vercel_project_id, vercel_team_id, supabase_ref)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(slug) DO UPDATE SET
                 name = excluded.name,
                 description = excluded.description,
                 local_path = excluded.local_path,
                 repo = excluded.repo,
                 production_url = excluded.production_url,
                 notes = excluded.notes,
                 vercel_project_id = excluded.vercel_project_id,
                 vercel_team_id = excluded.vercel_team_id,
                 supabase_ref = excluded.supabase_ref,
                 updated_at = datetime('now')",
            params![
                p.slug,
                p.name,
                p.description,
                p.local_path,
                p.repo,
                p.production_url,
                p.notes,
                p.vercel_project_id,
                p.vercel_team_id,
                p.supabase_ref
            ],
        )?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM projects ORDER BY slug"))?;
        let rows = stmt.query_map([], from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn get(&self, slug: &str) -> Result<Option<Project>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM projects WHERE slug = ?1"),
                [slug],
                from_row,
            )
            .optional()?)
    }

    /// Quita el proyecto y su historial de revisiones. Devuelve `true` si existía.
    pub fn remove(&self, slug: &str) -> Result<bool> {
        self.conn.execute("DELETE FROM checks WHERE slug = ?1", [slug])?;
        self.conn.execute("DELETE FROM events WHERE slug = ?1", [slug])?;
        self.conn.execute("DELETE FROM acks WHERE slug = ?1", [slug])?;
        self.conn.execute("DELETE FROM decisions WHERE slug = ?1", [slug])?;
        self.conn.execute(
            "DELETE FROM radar_events WHERE competitor_id IN (SELECT id FROM competitors WHERE slug = ?1)",
            [slug],
        )?;
        self.conn.execute("DELETE FROM competitors WHERE slug = ?1", [slug])?;
        self.conn.execute("DELETE FROM improvements WHERE slug = ?1", [slug])?;
        Ok(self
            .conn
            .execute("DELETE FROM projects WHERE slug = ?1", [slug])?
            > 0)
    }
}

fn from_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        slug: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        local_path: r.get(3)?,
        repo: r.get(4)?,
        production_url: r.get(5)?,
        notes: r.get(6)?,
        updated_at: r.get(7)?,
        vercel_project_id: r.get(8)?,
        vercel_team_id: r.get(9)?,
        supabase_ref: r.get(10)?,
    })
}

/// Resultado de una revisión de un proyecto.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Check {
    pub slug: String,
    /// `sitio`, `git` o `dependencias`.
    pub kind: String,
    /// `ok`, `aviso` o `falla`.
    pub status: String,
    pub summary: String,
    pub details: serde_json::Value,
    /// Fecha y hora en UTC.
    #[serde(default)]
    pub checked_at: String,
    /// Ya se reconoció este problema: sigue ahí, pero no cuenta como alerta abierta.
    #[serde(default)]
    pub acked: bool,
}

/// Una mejora pendiente o hecha de un proyecto.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Improvement {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub detail: String,
    /// `propuesta`, `aceptada`, `en_curso`, `en_revision`, `hecha` o `descartada`.
    pub status: String,
    /// Rama donde el agente dejó el trabajo.
    #[serde(default)]
    pub branch: Option<String>,
    /// Pull request abierto para revisarlo.
    #[serde(default)]
    pub pr_url: Option<String>,
    /// Qué hizo el agente, o por qué no pudo.
    #[serde(default)]
    pub result: Option<String>,
    /// Lo que el agente preguntó y lo que el usuario le respondió, para el siguiente intento.
    #[serde(default)]
    pub feedback: Option<String>,
    /// Quién la creó: `app`, `terminal` o `mcp`.
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

/// El flujo de una mejora: se propone, se acepta, el agente la trabaja en una rama
/// aparte, queda en revisión con su pull request y pasa a hecha al fusionarse.
pub const IMPROVEMENT_STATES: [&str; 6] =
    ["propuesta", "aceptada", "en_curso", "en_revision", "hecha", "descartada"];

impl Registry {
    pub(crate) fn init_checks(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS checks (
                 id         INTEGER PRIMARY KEY,
                 slug       TEXT NOT NULL,
                 kind       TEXT NOT NULL,
                 status     TEXT NOT NULL,
                 summary    TEXT NOT NULL,
                 details    TEXT NOT NULL,
                 checked_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE IF NOT EXISTS acks (
                 slug   TEXT NOT NULL,
                 kind   TEXT NOT NULL,
                 status TEXT NOT NULL,
                 at     TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (slug, kind)
             );
             CREATE TABLE IF NOT EXISTS improvements (
                 id         INTEGER PRIMARY KEY,
                 slug       TEXT NOT NULL,
                 title      TEXT NOT NULL,
                 detail     TEXT NOT NULL DEFAULT '',
                 status     TEXT NOT NULL DEFAULT 'propuesta',
                 source     TEXT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE IF NOT EXISTS events (
                 id     INTEGER PRIMARY KEY,
                 at     TEXT NOT NULL DEFAULT (datetime('now')),
                 slug   TEXT NOT NULL,
                 what   TEXT NOT NULL,
                 source TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS settings (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS checks_latest ON checks (slug, kind, id);
             DELETE FROM checks WHERE checked_at < datetime('now', '-30 days');",
        )?;
        Ok(())
    }

    /// Anota una acción hecha desde Atalaya (quién, sobre qué proyecto y qué hizo).
    pub fn record_event(&self, slug: &str, what: &str, source: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (slug, what, source) VALUES (?1, ?2, ?3)",
            [slug, what, source],
        )?;
        Ok(())
    }

    /// Últimas acciones de un proyecto: (fecha UTC, qué, origen).
    pub fn events(&self, slug: &str, limit: u32) -> Result<Vec<(String, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT at, what, source FROM events WHERE slug = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![slug, limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// Marcas en UTC para consultar periodos: inicio del mes, 14 días atrás y hoy,
    /// las tres a medianoche.
    pub fn utc_marks(&self) -> Result<(String, String, String)> {
        Ok(self.conn.query_row(
            "SELECT strftime('%Y-%m-01T00:00:00Z', 'now'),
                    strftime('%Y-%m-%dT00:00:00Z', 'now', '-14 days'),
                    strftime('%Y-%m-%dT00:00:00Z', 'now')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
    }

    /// Marca local para nombrar carpetas ("2026-11-23_0930") y la fecha de hoy.
    pub fn local_stamp(&self) -> Result<(String, String)> {
        Ok(self.conn.query_row(
            "SELECT strftime('%Y-%m-%d_%H%M', 'now', 'localtime'), date('now', 'localtime')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    pub fn utc_now(&self) -> Result<String> {
        Ok(self.conn.query_row("SELECT datetime('now')", [], |r| r.get(0))?)
    }

    /// Segundos transcurridos desde una fecha UTC guardada.
    pub fn seconds_since(&self, utc: &str) -> Result<f64> {
        Ok(self.conn.query_row(
            "SELECT (julianday('now') - julianday(?1)) * 86400.0",
            [utc],
            |r| r.get(0),
        )?)
    }

    /// Fecha (AAAA-MM-DD) y hora del día en la zona horaria de esta máquina.
    pub fn local_now(&self) -> Result<(String, u32)> {
        Ok(self.conn.query_row(
            "SELECT date('now', 'localtime'), CAST(strftime('%H', 'now', 'localtime') AS INTEGER)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }

    pub fn record_check(&self, c: &Check) -> Result<()> {
        self.conn.execute(
            "INSERT INTO checks (slug, kind, status, summary, details) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![c.slug, c.kind, c.status, c.summary, c.details.to_string()],
        )?;
        Ok(())
    }

    /// La revisión más reciente de cada tipo para un proyecto.
    pub fn latest_checks(&self, slug: &str) -> Result<Vec<Check>> {
        let mut stmt = self.conn.prepare(
            "SELECT slug, kind, status, summary, details, checked_at,
                    EXISTS (SELECT 1 FROM acks a
                            WHERE a.slug = c.slug AND a.kind = c.kind AND a.status = c.status)
             FROM checks c
             WHERE slug = ?1
               AND id = (SELECT max(id) FROM checks WHERE slug = c.slug AND kind = c.kind)
             ORDER BY kind",
        )?;
        let rows = stmt.query_map([slug], |r| {
            let details: String = r.get(4)?;
            Ok(Check {
                slug: r.get(0)?,
                kind: r.get(1)?,
                status: r.get(2)?,
                summary: r.get(3)?,
                details: serde_json::from_str(&details).unwrap_or_default(),
                checked_at: r.get(5)?,
                acked: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Historia del sitio por día: (día, revisiones, revisiones en orden, ms promedio).
    pub fn site_history(&self, slug: &str) -> Result<Vec<(String, u32, u32, Option<f64>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT date(checked_at, 'localtime'), count(*), sum(status != 'falla'),
                    avg(json_extract(details, '$.ms'))
             FROM checks WHERE slug = ?1 AND kind = 'sitio'
             GROUP BY 1 ORDER BY 1",
        )?;
        let rows = stmt.query_map([slug], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Reconoce el problema actual de una revisión. Vale mientras el estado no cambie:
    /// si empeora o se recupera y vuelve a fallar, cuenta como alerta nueva.
    pub fn acknowledge(&self, slug: &str, kind: &str) -> Result<()> {
        let (status, _) = self
            .last_check(slug, kind)?
            .with_context(|| format!("'{slug}' no tiene una revisión de tipo '{kind}'"))?;
        self.conn.execute(
            "INSERT INTO acks (slug, kind, status) VALUES (?1, ?2, ?3)
             ON CONFLICT(slug, kind) DO UPDATE SET status = excluded.status, at = datetime('now')",
            [slug, kind, status.as_str()],
        )?;
        Ok(())
    }

    pub fn unacknowledge(&self, slug: &str, kind: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM acks WHERE slug = ?1 AND kind = ?2", [slug, kind])?;
        Ok(())
    }

    pub fn add_improvement(
        &self,
        slug: &str,
        title: &str,
        detail: &str,
        source: &str,
    ) -> Result<i64> {
        let title = title.trim();
        if title.is_empty() {
            anyhow::bail!("la mejora necesita un título");
        }
        self.get(slug)?
            .with_context(|| format!("no existe el proyecto '{slug}'"))?;
        self.conn.execute(
            "INSERT INTO improvements (slug, title, detail, source) VALUES (?1, ?2, ?3, ?4)",
            [slug, title, detail.trim(), source],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Una mejora por su número.
    pub fn improvement(&self, id: i64) -> Result<Improvement> {
        self.improvements(None)?
            .into_iter()
            .find(|i| i.id == id)
            .with_context(|| format!("no existe la mejora {id}"))
    }

    /// Anota el avance del agente en una mejora. Lo que se pasa como `None` no cambia.
    pub fn update_improvement_run(
        &self,
        id: i64,
        status: &str,
        branch: Option<&str>,
        pr_url: Option<&str>,
        result: Option<&str>,
    ) -> Result<()> {
        self.set_improvement_status(id, status)?;
        self.conn.execute(
            "UPDATE improvements SET branch = coalesce(?2, branch), pr_url = coalesce(?3, pr_url),
                                     result = coalesce(?4, result)
             WHERE id = ?1",
            params![id, branch, pr_url, result],
        )?;
        Ok(())
    }

    /// Mejoras de un proyecto (o de todos), las más recientes primero.
    pub fn improvements(&self, slug: Option<&str>) -> Result<Vec<Improvement>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, slug, title, detail, status, source, created_at, updated_at,
                    branch, pr_url, result, feedback
             FROM improvements WHERE ?1 IS NULL OR slug = ?1 ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([slug], |r| {
            Ok(Improvement {
                id: r.get(0)?,
                slug: r.get(1)?,
                title: r.get(2)?,
                detail: r.get(3)?,
                status: r.get(4)?,
                source: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
                branch: r.get(8)?,
                pr_url: r.get(9)?,
                result: r.get(10)?,
                feedback: r.get(11)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_improvement_status(&self, id: i64, status: &str) -> Result<()> {
        if !IMPROVEMENT_STATES.contains(&status) {
            anyhow::bail!("estado inválido '{status}'; usa {}", IMPROVEMENT_STATES.join(", "));
        }
        let changed = self.conn.execute(
            "UPDATE improvements SET status = ?2, updated_at = datetime('now') WHERE id = ?1",
            params![id, status],
        )?;
        if changed == 0 {
            anyhow::bail!("no existe la mejora {id}");
        }
        Ok(())
    }

    /// El usuario le responde al agente (una pregunta, o más indicaciones tras un intento):
    /// queda anotado con lo que el agente dijo, y la mejora vuelve a "aceptada" para seguir.
    pub fn answer_improvement(&self, id: i64, answer: &str) -> Result<Improvement> {
        let answer = answer.trim();
        if answer.is_empty() {
            anyhow::bail!("escribe tu respuesta para el agente");
        }
        let improvement = self.improvement(id)?;
        if matches!(improvement.status.as_str(), "en_curso" | "hecha" | "descartada") {
            anyhow::bail!("la mejora {id} está '{}': no espera respuesta", improvement.status);
        }
        let said = improvement.result.as_deref().map(str::trim).filter(|r| !r.is_empty());
        let turn = match said {
            Some(said) => format!("El agente dijo:\n{said}\n\nEl usuario respondió:\n{answer}"),
            None => format!("El usuario agregó:\n{answer}"),
        };
        let feedback = match improvement.feedback.as_deref().filter(|f| !f.is_empty()) {
            Some(before) => format!("{before}\n\n---\n\n{turn}"),
            None => turn,
        };
        self.conn.execute(
            "UPDATE improvements SET feedback = ?2, status = 'aceptada', updated_at = datetime('now') WHERE id = ?1",
            params![id, feedback],
        )?;
        self.improvement(id)
    }

    /// Las mejoras que una conversación dejó "en curso" al terminar vuelven a "aceptada", con
    /// la nota: nadie las está trabajando y en el tablero no deben parecer en marcha.
    pub fn release_improvements(&self, ids: &[i64], note: &str) -> Result<Vec<i64>> {
        let mut released = Vec::new();
        for &id in ids {
            let Ok(improvement) = self.improvement(id) else { continue };
            if improvement.status == "en_curso" {
                self.update_improvement_run(id, "aceptada", None, None, Some(note))?;
                released.push(id);
            }
        }
        Ok(released)
    }

    pub fn delete_improvement(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM improvements WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Estado de la última revisión de ese tipo y cuántos segundos han pasado.
    pub fn last_check(&self, slug: &str, kind: &str) -> Result<Option<(String, f64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT status, (julianday('now') - julianday(checked_at)) * 86400.0
                 FROM checks WHERE slug = ?1 AND kind = ?2 ORDER BY id DESC LIMIT 1",
                [slug, kind],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use crate::testutil;

    #[test]
    fn a_conversation_only_releases_what_it_left_in_progress() {
        let r = testutil::registry("liberar");
        testutil::project(&r, "demo");
        let open = r.add_improvement("demo", "Quedó a medias", "", "mcp").unwrap();
        let done = r.add_improvement("demo", "Terminada", "", "mcp").unwrap();
        let other = r.add_improvement("demo", "La trabaja el agente de Mejoras", "", "app").unwrap();
        r.update_improvement_run(open, "en_curso", None, None, Some("Empezando")).unwrap();
        r.update_improvement_run(done, "en_revision", Some("mejora/2"), None, Some("Listo")).unwrap();
        r.set_improvement_status(other, "en_curso").unwrap();

        let released = r.release_improvements(&[open, done, 999], "Quedó a medias").unwrap();
        assert_eq!(released, vec![open]);
        let open = r.improvement(open).unwrap();
        assert_eq!((open.status.as_str(), open.result.as_deref()), ("aceptada", Some("Quedó a medias")));
        let done = r.improvement(done).unwrap();
        assert_eq!((done.status.as_str(), done.branch.as_deref()), ("en_revision", Some("mejora/2")));
        assert_eq!(r.improvement(other).unwrap().status, "en_curso");
    }

    #[test]
    fn answering_the_agent_keeps_the_conversation_for_the_next_attempt() {
        let r = testutil::registry("responder");
        testutil::project(&r, "demo");
        let id = r.add_improvement("demo", "Cambiar el color", "", "app").unwrap();
        r.update_improvement_run(id, "aceptada", None, None, Some("El agente no hizo cambios. Pregunta: ¿azul o verde?"))
            .unwrap();
        assert!(r.answer_improvement(id, "  ").is_err());
        let answered = r.answer_improvement(id, "Verde").unwrap();
        assert_eq!(answered.status, "aceptada");
        let feedback = answered.feedback.unwrap();
        assert!(feedback.contains("¿azul o verde?") && feedback.ends_with("El usuario respondió:\nVerde"));

        r.update_improvement_run(id, "en_revision", None, None, Some("Listo en verde")).unwrap();
        let again = r.answer_improvement(id, "Un tono más oscuro").unwrap();
        let feedback = again.feedback.unwrap();
        assert!(feedback.contains("Verde") && feedback.contains("---") && feedback.ends_with("Un tono más oscuro"));
        assert_eq!(again.status, "aceptada");

        r.set_improvement_status(id, "en_curso").unwrap();
        assert!(r.answer_improvement(id, "Otra cosa").is_err());
    }
}
