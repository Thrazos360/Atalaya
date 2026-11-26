//! Lo que ve la IA y la app de un proyecto: registro más inspección en vivo.

use crate::{Project, Registry, inspect};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::path::Path;

fn find(registry: &Registry, slug: &str) -> Result<Project> {
    registry.get(slug)?.with_context(|| {
        format!("no existe el proyecto '{slug}'; usa list_projects para ver los disponibles")
    })
}

/// Datos del registro, inspección en vivo de la carpeta (si está en esta máquina)
/// y el resultado de las últimas revisiones.
pub fn project_context(registry: &Registry, slug: &str) -> Result<Value> {
    let project = find(registry, slug)?;
    let live = match project.local_path.as_deref().map(Path::new) {
        Some(p) if p.is_dir() => {
            let mut live = inspect(p);
            live.services = crate::inspect::project_services(&project);
            json!(live)
        }
        Some(_) => json!({ "error": "la carpeta local no existe en esta máquina" }),
        None => Value::Null,
    };
    let checks = registry.latest_checks(slug)?;
    Ok(json!({ "project": project, "live": live, "checks": checks }))
}

pub fn recent_commits(registry: &Registry, slug: &str, limit: usize) -> Result<Value> {
    let project = find(registry, slug)?;
    let path = project
        .local_path
        .context("el proyecto no tiene carpeta local registrada")?;
    let git = crate::inspect::git_info(Path::new(&path), limit)
        .context("la carpeta no es un repositorio git")?;
    Ok(json!(git))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!("{}…", text.chars().take(max).collect::<String>())
}

/// El paquete de contexto común de un proyecto, igual para cualquier modelo: qué es, cómo
/// están sus revisiones, qué mejoras y decisiones tiene abiertas y qué pasó hace poco.
/// Los modelos por API lo reciben en JSON; Claude Code además puede usar el MCP.
pub fn pack(registry: &Registry, slug: &str) -> Result<Value> {
    let project = find(registry, slug)?;
    let checks: Vec<Value> = registry
        .latest_checks(slug)?
        .into_iter()
        .map(|c| json!({ "kind": c.kind, "status": c.status, "summary": clip(&c.summary, 300), "checked_at": c.checked_at }))
        .collect();
    let improvements: Vec<Value> = registry
        .improvements(Some(slug))?
        .into_iter()
        .filter(|i| !matches!(i.status.as_str(), "hecha" | "descartada"))
        .map(|i| json!({ "id": i.id, "title": i.title, "status": i.status, "detail": clip(&i.detail, 400) }))
        .collect();
    let decisions: Vec<Value> = crate::decisions::list(registry, true)?
        .into_iter()
        .filter(|d| d.slug == slug)
        .map(|d| json!({ "id": d.id, "title": d.title, "action": d.action }))
        .collect();
    let events: Vec<Value> = registry
        .events(slug, 10)?
        .into_iter()
        .map(|(at, what, source)| json!({ "at": at, "what": what, "source": source }))
        .collect();
    Ok(json!({
        "project": {
            "slug": project.slug,
            "name": project.name,
            "description": project.description,
            "repo": project.repo,
            "production_url": project.production_url,
            "notes": project.notes,
        },
        "checks": checks,
        "open_improvements": improvements,
        "pending_decisions": decisions,
        "recent_events": events,
    }))
}

/// El paquete de todos los proyectos, para el validador.
pub fn portfolio(registry: &Registry) -> Result<Value> {
    let mut list = Vec::new();
    for project in registry.list()? {
        list.push(pack(registry, &project.slug)?);
    }
    Ok(json!({ "projects": list }))
}
