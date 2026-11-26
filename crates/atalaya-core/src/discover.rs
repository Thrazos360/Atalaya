//! Carpetas de proyectos: cada persona elige dónde guarda sus proyectos, Atalaya los
//! busca ahí (una carpeta con `.git` es un proyecto), los registra y propone con qué
//! conexiones vincular cada uno según lo que encuentra en el repo.

use crate::connections::{self, Connection};
use crate::{Project, Registry};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Ajuste con la lista de carpetas raíz, en JSON.
pub const ROOTS: &str = "project_roots";
/// Hasta qué profundidad se busca bajo cada carpeta raíz.
const MAX_DEPTH: usize = 3;
/// Carpetas que nunca contienen proyectos propios y pueden ser enormes.
const SKIP: [&str; 9] = ["node_modules", "target", "dist", "build", "vendor", "Library", "Pods", "venv", "__pycache__"];

pub fn roots(registry: &Registry) -> Result<Vec<PathBuf>> {
    let saved = registry.setting(ROOTS)?.unwrap_or_default();
    let list: Vec<String> = serde_json::from_str(&saved).unwrap_or_default();
    Ok(list.into_iter().map(PathBuf::from).collect())
}

fn save_roots(registry: &Registry, list: &[PathBuf]) -> Result<()> {
    let text: Vec<String> = list.iter().map(|p| p.to_string_lossy().to_string()).collect();
    registry.set_setting(ROOTS, &serde_json::to_string(&text)?)
}

/// Agrega una carpeta raíz. Devuelve la ruta tal como quedó guardada.
pub fn add_root(registry: &Registry, path: &Path) -> Result<PathBuf> {
    let path = expand_home(path)
        .canonicalize()
        .with_context(|| format!("no existe la carpeta {}", path.display()))?;
    if !path.is_dir() {
        bail!("{} no es una carpeta", path.display());
    }
    let mut list = roots(registry)?;
    if !list.contains(&path) {
        list.push(path.clone());
        save_roots(registry, &list)?;
        registry.record_event("atalaya", &format!("carpeta de proyectos agregada: {}", path.display()), "carpetas")?;
    }
    Ok(path)
}

/// Quita una carpeta raíz de la lista. No toca la carpeta ni los proyectos registrados.
pub fn remove_root(registry: &Registry, path: &Path) -> Result<bool> {
    let wanted = expand_home(path);
    let wanted = wanted.canonicalize().unwrap_or(wanted);
    let mut list = roots(registry)?;
    let before = list.len();
    list.retain(|p| p != &wanted);
    if list.len() == before {
        return Ok(false);
    }
    save_roots(registry, &list)?;
    Ok(true)
}

fn expand_home(path: &Path) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix("~/")) {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| path.to_path_buf()),
        None => path.to_path_buf(),
    }
}

/// Carpeta donde clonar repos nuevos: la primera raíz elegida, la que comparten los
/// proyectos ya registrados o `~/Proyectos`.
pub fn clone_home(registry: &Registry) -> Result<PathBuf> {
    if let Some(first) = roots(registry)?.into_iter().find(|p| p.is_dir()) {
        return Ok(first);
    }
    let mut counts: std::collections::HashMap<PathBuf, usize> = std::collections::HashMap::new();
    for project in registry.list()? {
        if let Some(parent) = project.local_path.as_deref().and_then(|p| Path::new(p).parent()) {
            *counts.entry(parent.to_path_buf()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(dir, _)| dir)
        .or_else(|| dirs::home_dir().map(|h| h.join("Proyectos")))
        .context("no se encontró dónde clonar")
}

/// Carpetas con `.git` bajo `root`, sin entrar en un proyecto ya encontrado.
pub fn find_repos(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, 0, &mut found);
    found.sort();
    found
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if dir.join(".git").exists() {
        found.push(dir.to_path_buf());
        return;
    }
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || SKIP.contains(&name.as_str()) {
            continue;
        }
        // Los enlaces simbólicos se saltan: pueden formar ciclos o salir de la raíz.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&entry.path(), depth + 1, found);
        }
    }
}

/// Un vínculo que Atalaya propone a partir de lo que vio en el repo.
#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub role: String,
    pub connection_id: String,
    pub connection: String,
    pub config: Value,
    /// Por qué se propone, en palabras simples.
    pub reason: String,
}

/// Un proyecto encontrado en una carpeta raíz.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub path: String,
    pub name: String,
    pub slug: String,
    /// El proyecto registrado que ya usa esta carpeta, si hay uno.
    pub registered: Option<String>,
    pub remote: Option<String>,
    pub services: Vec<String>,
    pub suggestions: Vec<Suggestion>,
}

fn remote_of(path: &Path) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(path).args(["remote", "get-url", "origin"]).output().ok()?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !url.is_empty()).then_some(url)
}

/// El sitio de Netlify enlazado con `netlify link`.
fn netlify_site(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path.join(".netlify/state.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v["siteId"].as_str().map(String::from)
}

/// Busca la conexión de un proveedor con la que proponer un vínculo: la única que hay o
/// la predeterminada (la más antigua).
fn pick(all: &[Connection], provider: &str) -> Option<Connection> {
    all.iter().find(|c| c.provider == provider).cloned()
}

/// Vínculos que tendría sentido crear para la carpeta, según las conexiones que existen.
pub fn suggest(registry: &Registry, path: &Path, slug: Option<&str>) -> Result<Vec<Suggestion>> {
    let all = connections::all(registry)?;
    let services = crate::inspect::services(path);
    let mut out = Vec::new();
    let mut push = |role: &str, c: Connection, config: Value, reason: String| {
        let exists = slug.is_some_and(|s| connections::has_link(registry, s, role, &c.id).unwrap_or(false));
        if !exists {
            out.push(Suggestion { role: role.into(), connection_id: c.id.clone(), connection: c.name.clone(), config, reason });
        }
    };
    if let (Some(project_id), Some(c)) = (services.vercel_project_id.clone(), pick(&all, "vercel")) {
        let mut config = json!({ "project_id": project_id });
        if let Some(team) = services.vercel_team_id.clone().filter(|t| t.starts_with("team_")) {
            config["team_id"] = json!(team);
        }
        push("hosting", c, config, "la carpeta está enlazada con Vercel".into());
    }
    if let (Some(site), Some(c)) = (netlify_site(path), pick(&all, "netlify")) {
        push("hosting", c, json!({ "site_id": site }), "la carpeta está enlazada con Netlify".into());
    }
    if let (Some(reference), Some(c)) = (services.supabase_project_ref.clone(), pick(&all, "supabase")) {
        push("base", c, json!({ "ref": reference }), "el proyecto usa Supabase".into());
    }
    if let (Some(id), Some(c)) = (services.firebase_project.clone(), pick(&all, "firebase")) {
        push("base", c, json!({ "project": id }), "el proyecto usa Firebase".into());
    }
    if let Some(remote) = remote_of(path) {
        if let Some((host, repo)) = crate::providers::forge::parse_remote(&remote) {
            let forge = all.iter().find(|c| {
                matches!(c.provider.as_str(), "gitlab" | "gitea")
                    && c.config_str("url").is_some_and(|u| crate::providers::host_of(&u).to_lowercase() == host)
            });
            if let Some(c) = forge {
                push("repo", c.clone(), json!({ "repo": repo }), format!("el repo está en {host}"));
            }
        }
    }
    Ok(out)
}

/// Recorre las carpetas raíz y devuelve los proyectos encontrados, registrados o no.
pub fn scan(registry: &Registry) -> Result<Vec<Found>> {
    let registered: Vec<Project> = registry.list()?;
    let mut out = Vec::new();
    for root in roots(registry)? {
        for path in find_repos(&root) {
            let path_text = path.to_string_lossy().to_string();
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let known = registered.iter().find(|p| p.local_path.as_deref() == Some(path_text.as_str()));
            let services = crate::inspect::services(&path);
            let mut labels = Vec::new();
            if services.vercel_project_id.is_some() {
                labels.push("Vercel".to_string());
            }
            if netlify_site(&path).is_some() {
                labels.push("Netlify".to_string());
            }
            if services.supabase_project_ref.is_some() {
                labels.push("Supabase".to_string());
            }
            if services.firebase_project.is_some() {
                labels.push("Firebase".to_string());
            }
            out.push(Found {
                slug: known.map(|p| p.slug.clone()).unwrap_or_else(|| crate::registry::slugify(&name)),
                registered: known.map(|p| p.slug.clone()),
                remote: remote_of(&path),
                suggestions: suggest(registry, &path, known.map(|p| p.slug.as_str()))?,
                services: labels,
                name,
                path: path_text,
            });
        }
    }
    Ok(out)
}

/// Datos opcionales al registrar una carpeta. Lo que no se pasa se conserva si el
/// proyecto ya existía.
#[derive(Debug, Default, Clone)]
pub struct AddOptions {
    pub slug: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub notes: Option<String>,
    pub vercel_project: Option<String>,
    pub vercel_team: Option<String>,
    pub supabase_ref: Option<String>,
}

/// Registra (o actualiza) un proyecto a partir de su carpeta local.
pub fn add_local(registry: &Registry, path: &Path, options: AddOptions) -> Result<Project> {
    if let Some(r) = &options.supabase_ref {
        if !crate::inspect::is_supabase_ref(r) {
            bail!("'{r}' no parece una referencia de Supabase (20 letras minúsculas o dígitos)");
        }
    }
    let path = expand_home(path).canonicalize().with_context(|| format!("no existe la carpeta {}", path.display()))?;
    let folder = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let path_text = path.to_string_lossy().to_string();
    // Si la carpeta ya está registrada con otro slug, se actualiza ese proyecto.
    let same_folder = registry.list()?.into_iter().find(|p| p.local_path.as_deref() == Some(path_text.as_str()));
    let slug = options
        .slug
        .clone()
        .or_else(|| same_folder.as_ref().map(|p| p.slug.clone()))
        .unwrap_or_else(|| crate::registry::slugify(&folder));
    if slug.is_empty() {
        bail!("no se pudo deducir un identificador; pasa uno");
    }
    let old = registry.get(&slug)?;
    let old = old.as_ref();
    let keep = |new: Option<String>, get: fn(&Project) -> Option<String>| new.or_else(|| old.and_then(get));
    let project = Project {
        name: options.name.or_else(|| old.map(|o| o.name.clone())).unwrap_or(folder),
        description: keep(options.description, |o| o.description.clone()),
        repo: crate::inspect::git_info(&path, 0).and_then(|g| g.repo).or_else(|| old.and_then(|o| o.repo.clone())),
        production_url: keep(options.url, |o| o.production_url.clone()),
        notes: keep(options.notes, |o| o.notes.clone()),
        vercel_project_id: keep(options.vercel_project, |o| o.vercel_project_id.clone()),
        vercel_team_id: keep(options.vercel_team, |o| o.vercel_team_id.clone()),
        supabase_ref: keep(options.supabase_ref, |o| o.supabase_ref.clone()),
        local_path: Some(path_text),
        updated_at: String::new(),
        slug,
    };
    registry.upsert(&project)?;
    Ok(project)
}

/// Registra varias carpetas y crea los vínculos que se proponen para cada una.
/// Devuelve los proyectos registrados y cuántos vínculos se crearon.
pub fn register(registry: &Registry, paths: &[String], link: bool) -> Result<(Vec<Project>, usize)> {
    let mut projects = Vec::new();
    let mut links = 0;
    for path in paths {
        let project = add_local(registry, Path::new(path), AddOptions::default())?;
        if link {
            let local = PathBuf::from(project.local_path.clone().unwrap_or_default());
            for s in suggest(registry, &local, Some(&project.slug))? {
                connections::link(registry, &project.slug, &s.role, &s.connection_id, &s.config)?;
                links += 1;
            }
        }
        projects.push(project);
    }
    Ok((projects, links))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_repos_without_entering_them_or_skipped_folders() {
        let root = std::env::temp_dir().join(format!("atalaya-descubrir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in [
            "tienda/.git",
            "tienda/paquetes/interno/.git",
            "clientes/ana/web/.git",
            "clientes/ana/web2/.git",
            "node_modules/algo/.git",
            ".oculto/repo/.git",
            "muy/profundo/de/verdad/.git",
            "sin-git/src",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let found: Vec<String> = find_repos(&root)
            .iter()
            .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(found, vec!["clientes/ana/web", "clientes/ana/web2", "tienda"]);

        let registry = Registry::open(&root.join("atalaya.db")).unwrap();
        add_root(&registry, &root).unwrap();
        add_root(&registry, &root).unwrap();
        assert_eq!(roots(&registry).unwrap().len(), 1, "no se repite");
        let scanned = scan(&registry).unwrap();
        assert_eq!(scanned.len(), 3);
        assert!(scanned.iter().all(|f| f.registered.is_none()));

        let (projects, _) = register(&registry, &[scanned[2].path.clone()], true).unwrap();
        assert_eq!(projects[0].slug, "tienda");
        // Registrar otra vez la misma carpeta actualiza el mismo proyecto.
        let again = add_local(&registry, Path::new(&scanned[2].path), AddOptions::default()).unwrap();
        assert_eq!(again.slug, "tienda");
        assert_eq!(registry.list().unwrap().len(), 1);
        assert_eq!(scan(&registry).unwrap()[2].registered.as_deref(), Some("tienda"));
        assert_eq!(clone_home(&registry).unwrap(), root.canonicalize().unwrap());

        assert!(remove_root(&registry, &root).unwrap());
        assert!(roots(&registry).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
