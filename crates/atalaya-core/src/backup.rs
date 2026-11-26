//! Respaldos automáticos de las bases de datos.
//!
//! Una copia diaria por proyecto dentro de la carpeta de datos de Atalaya:
//! - Supabase: una tabla por archivo JSON; Firestore: todos los documentos en JSON.
//!   Son copias de lectura para recuperar datos, no un volcado restaurable con un comando.
//! - Bases propias: `pg_dump` (formato propio de Postgres), `mysqldump` (SQL) y una copia
//!   consistente del archivo en SQLite. Estas sí se restauran con sus herramientas.

use crate::{Check, Project, Registry, checks};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Cuántos respaldos se conservan por proyecto.
const KEEP: usize = 7;
/// Tablas más grandes que esto se omiten: no caben bien en una sola consulta.
const MAX_TABLE_BYTES: u64 = 200 * 1024 * 1024;

pub fn root() -> Result<PathBuf> {
    let base = Registry::default_path()?;
    Ok(base.parent().context("sin carpeta de datos")?.join("respaldos"))
}

fn supabase(token: &str, reference: &str, dir: &Path) -> Result<Value> {
    let tables = crate::supabase::query_with(
        token,
        reference,
        "select c.relname as tabla, pg_total_relation_size(c.oid) as bytes
         from pg_class c join pg_namespace n on n.oid = c.relnamespace
         where n.nspname = 'public' and c.relkind = 'r' order by 1",
    )?;
    let (mut saved, mut rows_total, mut skipped) = (0, 0u64, Vec::new());
    for table in &tables {
        let name = table["tabla"].as_str().unwrap_or_default();
        // El nombre viene del catálogo de Postgres; aun así se descarta lo que no sea un identificador simple.
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if table["bytes"].as_u64().unwrap_or(0) > MAX_TABLE_BYTES {
            skipped.push(name.to_string());
            continue;
        }
        let rows = crate::supabase::query_with(
            token,
            reference,
            &format!("select coalesce(json_agg(t), '[]'::json) as filas from public.\"{name}\" t"),
        )?;
        let data = rows.first().map(|r| r["filas"].clone()).unwrap_or_else(|| json!([]));
        rows_total += data.as_array().map(|a| a.len() as u64).unwrap_or(0);
        std::fs::write(dir.join(format!("{name}.json")), serde_json::to_vec(&data)?)?;
        saved += 1;
    }
    Ok(json!({ "origen": "supabase", "tablas": saved, "filas": rows_total, "omitidas": skipped }))
}

fn firestore(project: &str, dir: &Path) -> Result<Value> {
    let docs = crate::firebase::all_documents(project)?;
    let count = docs.len();
    std::fs::write(dir.join("firestore.json"), serde_json::to_vec(&docs)?)?;
    Ok(json!({ "origen": "firestore", "documentos": count }))
}

fn own_databases(
    registry: &Registry,
    slug: &str,
    role: &str,
) -> Vec<(crate::connections::Link, crate::connections::Connection)> {
    crate::connections::linked(registry, slug, role, None)
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, c)| matches!(c.provider.as_str(), "postgres" | "mysql" | "sqlite"))
        .collect()
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| e.ok()?.metadata().ok()).map(|m| m.len()).sum())
        .unwrap_or(0)
}

/// Respalda las bases de un proyecto. `None` si no tiene ninguna base conocida y con llave.
pub fn run(registry: &Registry, project: &Project) -> Option<Result<Value>> {
    let services = crate::inspect::project_services(project);
    let supabase_link = checks::supabase_ref(registry, project)
        .and_then(|r| Some((crate::connections::token_for(registry, Some(&project.slug), "supabase")?, r)));
    let firebase_id = services.firebase_project.filter(|_| crate::firebase::connected());
    // Bases propias: las vinculadas como respaldo o, si no hay ninguna, las vinculadas como base.
    let mut own = own_databases(registry, &project.slug, "respaldo");
    if own.is_empty() {
        own = own_databases(registry, &project.slug, "base");
    }
    if supabase_link.is_none() && firebase_id.is_none() && own.is_empty() {
        return None;
    }
    Some((|| {
        let (stamp, _) = registry.local_stamp()?;
        let home = root()?.join(&project.slug);
        let dir = home.join(&stamp);
        std::fs::create_dir_all(&dir)?;
        let mut parts = Vec::new();
        let outcome = (|| -> Result<()> {
            if let Some((token, reference)) = &supabase_link {
                parts.push(supabase(token, reference, &dir)?);
            }
            for (link, c) in &own {
                parts.push(crate::providers::database::backup(c, link, &dir)?);
            }
            if let Some(id) = &firebase_id {
                parts.push(firestore(id, &dir)?);
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            // Un respaldo a medias confunde más de lo que ayuda.
            let _ = std::fs::remove_dir_all(&dir);
            return Err(e);
        }
        let bytes = dir_size(&dir);
        if bytes == 0 {
            let _ = std::fs::remove_dir_all(&dir);
            bail!("el respaldo quedó vacío");
        }
        let meta = json!({ "fecha": stamp, "bytes": bytes, "partes": parts, "carpeta": dir.display().to_string() });
        std::fs::write(dir.join("_respaldo.json"), serde_json::to_vec_pretty(&meta)?)?;

        // Se conservan los más recientes; los nombres ordenan por fecha.
        let mut old: Vec<PathBuf> = std::fs::read_dir(&home)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();
        old.sort();
        for stale in old.iter().rev().skip(KEEP) {
            let _ = std::fs::remove_dir_all(stale);
        }
        Ok(meta)
    })())
}

fn describe(meta: &Value) -> String {
    let parts: Vec<String> = meta["partes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| match p["origen"].as_str() {
            Some("supabase") => format!("{} tablas y {} filas de Supabase", p["tablas"], p["filas"]),
            Some("firestore") => format!("{} documentos de Firestore", p["documentos"]),
            Some(kind) => format!("{} ({kind})", p["conexion"].as_str().unwrap_or("base")),
            None => "una base".to_string(),
        })
        .collect();
    let mb = meta["bytes"].as_u64().unwrap_or(0) as f64 / 1_048_576.0;
    format!("Respaldo de {} ({mb:.1} MB)", parts.join(" y "))
}

/// Respalda un proyecto y deja el resultado como revisión `respaldo`.
/// Devuelve la revisión y el estado anterior, igual que las demás revisiones.
pub fn backup_project(registry: &Registry, project: &Project) -> Result<Option<(Check, Option<String>)>> {
    let Some(outcome) = run(registry, project) else { return Ok(None) };
    let previous = registry.last_check(&project.slug, "respaldo")?.map(|(status, _)| status);
    let (status, summary, details) = match outcome {
        Ok(meta) => (checks::OK, describe(&meta), meta),
        Err(e) => (checks::FAIL, "El respaldo falló".to_string(), json!({ "error": format!("{e:#}") })),
    };
    let check = Check {
        slug: project.slug.clone(),
        kind: "respaldo".into(),
        status: status.into(),
        summary,
        details,
        checked_at: String::new(),
        acked: false,
    };
    registry.record_check(&check)?;
    Ok(Some((check, previous)))
}

/// Ronda del vigía: un respaldo al día por proyecto.
pub fn round(registry: &Registry) -> Result<Vec<(Check, Option<String>)>> {
    let mut done = Vec::new();
    for project in registry.list()? {
        let due = registry
            .last_check(&project.slug, "respaldo")?
            .is_none_or(|(status, age)| age >= 86400.0 || (status == checks::FAIL && age >= 3600.0));
        if due {
            done.extend(backup_project(registry, &project)?);
        }
    }
    Ok(done)
}
