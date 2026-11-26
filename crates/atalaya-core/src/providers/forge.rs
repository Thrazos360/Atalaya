//! Repositorios fuera de GitHub: GitLab (el de gitlab.com o uno propio) y Gitea/Forgejo.

use super::{Outcome, expect_2xx, http};
use crate::checks::{FAIL, OK, WARN};
use crate::connections::{Connection, Link};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

fn base_url(c: &Connection) -> Result<String> {
    Ok(c.config_str("url").context("la conexión no tiene dirección")?.trim_end_matches('/').to_string())
}

fn headers(c: &Connection) -> Result<Vec<(&'static str, String)>> {
    let token = c.secret().with_context(|| format!("la conexión «{}» no tiene llave guardada", c.name))?;
    Ok(match c.provider.as_str() {
        "gitlab" => vec![("PRIVATE-TOKEN", token)],
        _ => vec![("Authorization", format!("token {token}"))],
    })
}

fn api(c: &Connection, path: &str) -> Result<Value> {
    let (prefix, service) = match c.provider.as_str() {
        "gitlab" => ("/api/v4", "GitLab"),
        "gitea" => ("/api/v1", "Gitea"),
        other => bail!("'{other}' no es un repositorio"),
    };
    let url = format!("{}{prefix}{path}", base_url(c)?);
    expect_2xx(service, http("GET", &url, &headers(c)?, None, 30)?)
}

pub fn test(c: &Connection) -> Result<String> {
    let user = api(c, "/user")?;
    let login = user["username"].as_str().or(user["login"].as_str()).unwrap_or("sin nombre");
    Ok(format!("Conectado como {login} en {}", super::host_of(&base_url(c)?)))
}

/// `https://gitlab.com/grupo/sub/repo.git` o `git@gitlab.com:grupo/repo.git` → (`gitlab.com`, `grupo/sub/repo`).
pub fn parse_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    } else {
        let rest = url.split_once("://")?.1;
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?.to_string(), path.to_string())
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    (path.contains('/') && !host.is_empty()).then(|| (host.to_lowercase(), path.to_string()))
}

/// Estado del repo de un proyecto: último pipeline o comprobación y lo que está abierto.
pub fn check(c: &Connection, link: &Link) -> Option<Outcome> {
    let repo = link.config_str("repo")?;
    let result = match c.provider.as_str() {
        "gitlab" => gitlab(c, &repo),
        "gitea" => gitea(c, &repo),
        _ => return None,
    };
    Some(result.unwrap_or_else(|e| (WARN, "No se pudo consultar el repositorio".into(), json!({ "error": format!("{e:#}") }))))
}

fn encode(path: &str) -> String {
    path.replace('/', "%2F")
}

fn gitlab(c: &Connection, repo: &str) -> Result<Outcome> {
    let project = api(c, &format!("/projects/{}", encode(repo)))?;
    let id = project["id"].as_u64().context("GitLab no devolvió el proyecto")?;
    let pipeline = api(c, &format!("/projects/{id}/pipelines?per_page=1"))?;
    let state = pipeline[0]["status"].as_str().unwrap_or("");
    let open_mrs = api(c, &format!("/projects/{id}/merge_requests?state=opened&per_page=100"))?
        .as_array()
        .map_or(0, Vec::len);
    let issues = project["open_issues_count"].as_u64().unwrap_or(0);
    let details = json!({ "pipeline": state, "merge_requests": open_mrs, "issues": issues, "rama": project["default_branch"] });
    Ok(summarize(state, open_mrs as u64 + issues, details))
}

fn gitea(c: &Connection, repo: &str) -> Result<Outcome> {
    let info = api(c, &format!("/repos/{repo}"))?;
    let branch = info["default_branch"].as_str().unwrap_or("main");
    let status = api(c, &format!("/repos/{repo}/commits/{branch}/status")).unwrap_or(Value::Null);
    let state = status["state"].as_str().unwrap_or("");
    let open = info["open_issues_count"].as_u64().unwrap_or(0) + info["open_pr_counter"].as_u64().unwrap_or(0);
    let details = json!({ "estado": state, "abiertos": open, "rama": branch });
    Ok(summarize(state, open, details))
}

fn summarize(state: &str, open: u64, details: Value) -> Outcome {
    let (status, mut summary) = match state {
        "failed" | "failure" | "error" => (FAIL, "El último pipeline falló".to_string()),
        "running" | "pending" | "created" => (OK, "Pipeline en curso".to_string()),
        "success" => (OK, "Último pipeline correcto".to_string()),
        "canceled" => (WARN, "El último pipeline se canceló".to_string()),
        _ => (OK, "Sin pipelines".to_string()),
    };
    if open > 0 {
        summary.push_str(&format!(" · {open} abiertos"));
    }
    (status, summary, details)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes_from_any_host() {
        assert_eq!(
            parse_remote("https://gitlab.com/grupo/sub/repo.git"),
            Some(("gitlab.com".into(), "grupo/sub/repo".into()))
        );
        assert_eq!(parse_remote("git@git.casa.dev:ana/tienda.git"), Some(("git.casa.dev".into(), "ana/tienda".into())));
        assert_eq!(
            parse_remote("https://usuario@git.casa.dev:3000/ana/tienda"),
            Some(("git.casa.dev".into(), "ana/tienda".into()))
        );
        assert_eq!(parse_remote("https://gitlab.com/solo"), None);
    }

    #[test]
    fn pipeline_states_map_to_statuses() {
        assert_eq!(summarize("failed", 0, Value::Null).0, FAIL);
        assert_eq!(summarize("success", 2, Value::Null).1, "Último pipeline correcto · 2 abiertos");
        assert_eq!(summarize("", 0, Value::Null).0, OK);
    }
}
