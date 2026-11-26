//! GitHub: ver los repos de la cuenta, crear nuevos y vigilar los que no tienen
//! carpeta en esta máquina. La llave sale del Llavero (`github`) o, si no hay,
//! de la sesión del comando `gh`.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn find_gh() -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    from_path
        .into_iter()
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from))
        .map(|dir| dir.join("gh"))
        .find(|p| p.is_file())
}

pub fn token() -> Option<String> {
    if let Some(saved) = crate::secrets::get("github") {
        return Some(saved);
    }
    let out = Command::new(find_gh()?).args(["auth", "token"]).output().ok()?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !value.is_empty()).then_some(value)
}

/// Llamada a la API de GitHub. Devuelve código HTTP y cuerpo JSON.
pub fn api(method: &str, path: &str, body: Option<&Value>) -> Result<(u16, Value)> {
    let token = token().context(
        "falta acceso a GitHub: agrega la conexión de GitHub en Conexiones o inicia sesión con `gh auth login`",
    )?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let url = format!("https://api.github.com{path}");
    let auth = format!("Bearer {token}");
    let headers = [
        ("Authorization", auth.as_str()),
        ("Accept", "application/vnd.github+json"),
        ("User-Agent", "Atalaya"),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ];
    let mut response = match method {
        "GET" => {
            let mut request = agent.get(&url);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            request.call()
        }
        "POST" | "PUT" => {
            let mut request = if method == "POST" { agent.post(&url) } else { agent.put(&url) };
            for (name, value) in headers {
                request = request.header(name, value);
            }
            request
                .header("Content-Type", "application/json")
                .send(body.map(Value::to_string).unwrap_or_default())
        }
        other => bail!("método no soportado: {other}"),
    }
    .context("no se pudo conectar con GitHub")?;
    let parsed = response
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    Ok((response.status().as_u16(), parsed))
}

pub fn api_put(path: &str, body: &Value) -> Result<(u16, Value)> {
    api("PUT", path, Some(body))
}

fn expect_ok(what: &str, (code, body): (u16, Value)) -> Result<Value> {
    if code >= 400 {
        let message = body["message"].as_str().unwrap_or("sin detalle");
        bail!("GitHub rechazó {what} (error {code}): {message}");
    }
    Ok(body)
}

#[derive(Debug, Serialize)]
pub struct Repo {
    /// `dueño/repo`.
    pub full_name: String,
    pub name: String,
    pub description: Option<String>,
    pub private: bool,
    pub homepage: Option<String>,
    pub pushed_at: Option<String>,
    pub default_branch: String,
    pub url: String,
}

fn repo_from(v: &Value) -> Repo {
    let text = |key: &str| v[key].as_str().filter(|s| !s.is_empty()).map(String::from);
    Repo {
        full_name: text("full_name").unwrap_or_default(),
        name: text("name").unwrap_or_default(),
        description: text("description"),
        private: v["private"].as_bool().unwrap_or(false),
        homepage: text("homepage"),
        pushed_at: text("pushed_at"),
        default_branch: text("default_branch").unwrap_or_else(|| "main".into()),
        url: text("html_url").unwrap_or_default(),
    }
}

/// Repos de la cuenta, de sus organizaciones y en los que colabora; los de actividad
/// más reciente primero.
pub fn repos() -> Result<Vec<Repo>> {
    let body = expect_ok(
        "listar los repos",
        api("GET", "/user/repos?per_page=100&sort=pushed&affiliation=owner,collaborator,organization_member", None)?,
    )?;
    Ok(body.as_array().map(|list| list.iter().map(repo_from).collect()).unwrap_or_default())
}

/// Crea un repo nuevo en la cuenta, con un README inicial para que se pueda clonar.
pub fn create(name: &str, description: &str, private: bool) -> Result<Repo> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok {
        bail!("nombre inválido '{name}': usa letras, dígitos, guiones, puntos y guion bajo");
    }
    let body = json!({
        "name": name,
        "description": description,
        "private": private,
        "auto_init": true,
    });
    Ok(repo_from(&expect_ok("crear el repo", api("POST", "/user/repos", Some(&body))?)?))
}

/// Estado del repo en GitHub: resultado de las comprobaciones del último commit de la
/// rama principal, issues y PR abiertos, y los commits más recientes.
pub fn status(repo: &str) -> Result<Value> {
    let info = expect_ok("leer el repo", api("GET", &format!("/repos/{repo}"), None)?)?;
    let branch = info["default_branch"].as_str().unwrap_or("main").to_string();
    let commits = expect_ok(
        "leer los commits",
        api("GET", &format!("/repos/{repo}/commits?sha={branch}&per_page=8"), None)?,
    )?;
    let commits: Vec<Value> = commits
        .as_array()
        .map(|list| {
            list.iter()
                .map(|c| {
                    let message = c["commit"]["message"].as_str().unwrap_or_default();
                    json!({
                        "sha": c["sha"].as_str().unwrap_or_default().chars().take(7).collect::<String>(),
                        "fecha": c["commit"]["committer"]["date"].as_str().unwrap_or_default().chars().take(10).collect::<String>(),
                        "mensaje": message.lines().next().unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // "pending" también es lo que GitHub responde cuando el commit no tiene comprobaciones.
    let (_, combined) = api("GET", &format!("/repos/{repo}/commits/{branch}/status"), None)?;
    let checks = combined["total_count"].as_u64().unwrap_or(0);
    let state = if checks == 0 {
        "sin comprobaciones"
    } else {
        combined["state"].as_str().unwrap_or("desconocido")
    };
    Ok(json!({
        "rama": branch,
        "comprobaciones": state,
        "abiertos": info["open_issues_count"],
        "privado": info["private"],
        "commits": commits,
    }))
}

/// Issues abiertos de un repo (sin los pull request), los más recientes primero.
pub fn issues(repo: &str) -> Result<Value> {
    let body = expect_ok(
        "listar los issues",
        api("GET", &format!("/repos/{repo}/issues?state=open&per_page=30&sort=updated"), None)?,
    )?;
    Ok(Value::Array(
        body.as_array()
            .into_iter()
            .flatten()
            .filter(|i| i.get("pull_request").is_none())
            .map(|i| {
                json!({
                    "numero": i["number"],
                    "titulo": i["title"],
                    "autor": i["user"]["login"],
                    "texto": i["body"].as_str().unwrap_or_default().chars().take(1500).collect::<String>(),
                    "comentarios": i["comments"],
                    "actualizado": i["updated_at"],
                })
            })
            .collect(),
    ))
}

/// Clona el repo de un proyecto vigilado solo en remoto y le asigna esa carpeta local,
/// para que pase a tener interruptores, revisión de dependencias y agente.
pub fn clone(registry: &crate::Registry, slug: &str) -> Result<PathBuf> {
    let mut project = registry.get(slug)?.with_context(|| format!("no existe el proyecto '{slug}'"))?;
    if project.local_path.as_deref().is_some_and(|p| std::path::Path::new(p).is_dir()) {
        bail!("el proyecto ya tiene carpeta local");
    }
    let repo = project.repo.clone().context("el proyecto no tiene repo de GitHub")?;
    let home = crate::discover::clone_home(registry)?;
    std::fs::create_dir_all(&home)?;
    let target = home.join(repo.rsplit('/').next().unwrap_or(&repo));
    if target.exists() {
        bail!("ya existe la carpeta {}", target.display());
    }
    // Primero por SSH, como suelen estar configurados los repos; si falla, por HTTPS.
    let attempt = |url: String| {
        Command::new("git")
            .args(["clone", "--quiet", &url])
            .arg(&target)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
    };
    let mut out = attempt(format!("git@github.com:{repo}.git"))?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&target);
        out = attempt(format!("https://github.com/{repo}.git"))?;
    }
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&target);
        bail!("no se pudo clonar {repo}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    project.local_path = Some(target.to_string_lossy().to_string());
    registry.upsert(&project)?;
    registry.record_event(slug, &format!("repo clonado en {}", target.display()), "app")?;
    Ok(target)
}

/// Registra un repo de GitHub como proyecto sin carpeta local.
pub fn watch(
    registry: &crate::Registry,
    repo: &str,
    slug: Option<String>,
    url: Option<String>,
) -> Result<crate::Project> {
    let name = repo.rsplit('/').next().unwrap_or(repo).to_string();
    if repo.split('/').count() != 2 || name.is_empty() {
        bail!("'{repo}' no tiene la forma dueño/repo");
    }
    let taken = registry
        .list()?
        .iter()
        .any(|p| p.repo.as_deref().is_some_and(|r| r.eq_ignore_ascii_case(repo)));
    if taken {
        bail!("{repo} ya está registrado");
    }
    let slug = slug.unwrap_or_else(|| crate::registry::slugify(&name));
    if registry.get(&slug)?.is_some() {
        bail!("ya existe un proyecto con el identificador '{slug}'");
    }
    let project = crate::Project {
        slug,
        name,
        description: None,
        local_path: None,
        repo: Some(repo.to_string()),
        production_url: url.filter(|u| !u.is_empty()),
        notes: None,
        vercel_project_id: None,
        vercel_team_id: None,
        supabase_ref: None,
        updated_at: String::new(),
    };
    registry.upsert(&project)?;
    Ok(project)
}
