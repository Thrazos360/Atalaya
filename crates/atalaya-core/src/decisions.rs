//! Decisiones: la cola de cosas que esperan el visto bueno del usuario.
//!
//! El agente revisa el estado de los proyectos en su ronda y deja propuestas aquí.
//! Nada se ejecuta hasta que el usuario aprueba; al aprobar, Atalaya hace la acción
//! asociada (crear una mejora, comentar un issue, reconocer una alerta o poner al día la
//! carpeta local del proyecto).

use crate::Registry;
use anyhow::{Context, Result, bail};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Qué pasa al aprobar una decisión.
pub const ACTIONS: [&str; 5] = ["mejora", "comentario", "reconocer", "actualizar", "ninguna"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: i64,
    pub slug: String,
    /// De dónde sale: `alerta`, `issue`, `radar`, `mantenimiento`…
    pub kind: String,
    pub title: String,
    /// La evidencia: qué se observó.
    pub context: String,
    /// Lo que el agente propone hacer o decir. El usuario puede editarlo antes de aprobar.
    pub proposal: String,
    /// `mejora`, `comentario`, `reconocer`, `actualizar` (la carpeta local) o `ninguna`.
    pub action: String,
    /// Dato que necesita la acción: número del issue o tipo de revisión.
    pub target: Option<String>,
    /// `pendiente`, `aprobada` o `descartada`.
    pub status: String,
    pub result: Option<String>,
    pub created_at: String,
}

pub(crate) fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS decisions (
             id         INTEGER PRIMARY KEY,
             slug       TEXT NOT NULL,
             kind       TEXT NOT NULL,
             title      TEXT NOT NULL,
             context    TEXT NOT NULL DEFAULT '',
             proposal   TEXT NOT NULL DEFAULT '',
             action     TEXT NOT NULL DEFAULT 'ninguna',
             target     TEXT,
             status     TEXT NOT NULL DEFAULT 'pendiente',
             result     TEXT,
             created_at TEXT NOT NULL DEFAULT (datetime('now')),
             decided_at TEXT
         );",
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn propose(
    registry: &Registry,
    slug: &str,
    kind: &str,
    title: &str,
    context: &str,
    proposal: &str,
    action: &str,
    target: Option<&str>,
) -> Result<i64> {
    if !ACTIONS.contains(&action) {
        bail!("acción inválida '{action}'; usa {}", ACTIONS.join(", "));
    }
    if title.trim().is_empty() {
        bail!("la decisión necesita un título");
    }
    registry.get(slug)?.with_context(|| format!("no existe el proyecto '{slug}'"))?;
    // No se repite una propuesta que ya está esperando respuesta.
    let duplicate: bool = registry.conn().query_row(
        "SELECT count(*) > 0 FROM decisions WHERE slug = ?1 AND title = ?2 AND status = 'pendiente'",
        [slug, title.trim()],
        |r| r.get(0),
    )?;
    if duplicate {
        bail!("ya hay una decisión pendiente con ese título para '{slug}'");
    }
    registry.conn().execute(
        "INSERT INTO decisions (slug, kind, title, context, proposal, action, target)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![slug, kind, title.trim(), context.trim(), proposal.trim(), action, target],
    )?;
    Ok(registry.conn().last_insert_rowid())
}

/// Decisiones, las pendientes primero y dentro de cada grupo las más nuevas.
pub fn list(registry: &Registry, only_pending: bool) -> Result<Vec<Decision>> {
    let mut stmt = registry.conn().prepare(
        "SELECT id, slug, kind, title, context, proposal, action, target, status, result, created_at
         FROM decisions WHERE ?1 = 0 OR status = 'pendiente'
         ORDER BY status != 'pendiente', id DESC LIMIT 200",
    )?;
    let rows = stmt.query_map([only_pending], |r| {
        Ok(Decision {
            id: r.get(0)?,
            slug: r.get(1)?,
            kind: r.get(2)?,
            title: r.get(3)?,
            context: r.get(4)?,
            proposal: r.get(5)?,
            action: r.get(6)?,
            target: r.get(7)?,
            status: r.get(8)?,
            result: r.get(9)?,
            created_at: r.get(10)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn close(registry: &Registry, id: i64, status: &str, result: &str) -> Result<()> {
    registry.conn().execute(
        "UPDATE decisions SET status = ?2, result = ?3, decided_at = datetime('now') WHERE id = ?1",
        params![id, status, result],
    )?;
    Ok(())
}

pub fn discard(registry: &Registry, id: i64) -> Result<()> {
    close(registry, id, "descartada", "Descartada")
}

/// Cierra las decisiones pendientes de un proyecto con esa acción porque ya se resolvieron
/// solas (por ejemplo, la carpeta se puso al día en una vuelta del Vigía).
pub(crate) fn settle(registry: &Registry, slug: &str, action: &str, result: &str) -> Result<usize> {
    let closed = registry.conn().execute(
        "UPDATE decisions SET status = 'aprobada', result = ?3, decided_at = datetime('now')
         WHERE slug = ?1 AND action = ?2 AND status = 'pendiente'",
        params![slug, action, format!("Se resolvió sola: {result}")],
    )?;
    Ok(closed)
}

/// Aprueba una decisión y ejecuta su acción, con el texto final que dejó el usuario.
/// Con `work`, una decisión informativa se convierte en mejora para que el agente la
/// resuelva. Devuelve el número de la mejora creada, si hubo una, para que quien
/// llama la ponga en la cola del agente.
pub fn approve(registry: &Registry, id: i64, proposal: &str, work: bool) -> Result<Option<i64>> {
    let decision = list(registry, false)?
        .into_iter()
        .find(|d| d.id == id)
        .with_context(|| format!("no existe la decisión {id}"))?;
    if decision.status != "pendiente" {
        bail!("la decisión {id} ya está {}", decision.status);
    }
    let text = if proposal.trim().is_empty() { decision.proposal.as_str() } else { proposal.trim() };
    let mut improvement = None;
    let action = if work && decision.action == "ninguna" { "mejora" } else { decision.action.as_str() };
    let result = match action {
        "mejora" => {
            // El agente recibe también lo que se observó, no solo la propuesta.
            let brief = if decision.context.is_empty() {
                text.to_string()
            } else {
                format!("{text}\n\nLo que se observó: {}", decision.context)
            };
            let new = registry.add_improvement(&decision.slug, &decision.title, &brief, "agente")?;
            registry.set_improvement_status(new, "aceptada")?;
            improvement = Some(new);
            format!("Mejora #{new} creada y aceptada")
        }
        "comentario" => {
            let project = registry.get(&decision.slug)?.context("el proyecto ya no existe")?;
            let repo = project.repo.context("el proyecto no tiene repo de GitHub")?;
            let number = decision.target.as_deref().context("la decisión no dice en qué issue comentar")?;
            let (code, body) = crate::github::api(
                "POST",
                &format!("/repos/{repo}/issues/{number}/comments"),
                Some(&json!({ "body": text })),
            )?;
            if code >= 400 {
                bail!("GitHub rechazó el comentario (error {code}): {}", body["message"].as_str().unwrap_or("sin detalle"));
            }
            format!("Comentario publicado en el issue #{number}")
        }
        "actualizar" => crate::local_sync::now(registry, &decision.slug)?,
        "reconocer" => {
            let kind = decision.target.as_deref().context("la decisión no dice qué alerta reconocer")?;
            registry.acknowledge(&decision.slug, kind)?;
            format!("Alerta de {kind} reconocida")
        }
        _ => "Aprobada".to_string(),
    };
    registry.conn().execute("UPDATE decisions SET proposal = ?2 WHERE id = ?1", params![id, text])?;
    close(registry, id, "aprobada", &result)?;
    registry.record_event(&decision.slug, &format!("decisión #{id} aprobada: {result}"), "app")?;
    Ok(improvement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    fn registry(name: &str) -> (Registry, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("atalaya-dec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let registry = Registry::open(&dir.join("p.db")).unwrap();
        registry
            .upsert(&Project {
                slug: "demo".into(),
                name: "Demo".into(),
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
        (registry, dir)
    }

    #[test]
    fn approving_an_informative_decision_can_put_the_agent_to_work() {
        let (r, dir) = registry("trabajar");
        let seen = propose(&r, "demo", "alerta", "Solo mirar", "Contexto A", "Texto A", "ninguna", None).unwrap();
        let worked = propose(&r, "demo", "alerta", "Resolver esto", "Se vio X", "Haz Y", "ninguna", None).unwrap();
        assert!(propose(&r, "demo", "alerta", "Resolver esto", "", "", "ninguna", None).is_err(), "no se repite");

        // Darla por vista no crea nada.
        assert_eq!(approve(&r, seen, "", false).unwrap(), None);
        assert!(r.improvements(None).unwrap().is_empty());

        // Aprobarla para trabajarla crea una mejora aceptada, con propuesta y contexto.
        let id = approve(&r, worked, "Haz Y, editado", true).unwrap().expect("mejora");
        let improvement = r.improvement(id).unwrap();
        assert_eq!(improvement.status, "aceptada");
        assert!(improvement.detail.contains("Haz Y, editado") && improvement.detail.contains("Se vio X"));
        assert!(approve(&r, worked, "", true).is_err(), "ya estaba resuelta");
        assert!(list(&r, true).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
