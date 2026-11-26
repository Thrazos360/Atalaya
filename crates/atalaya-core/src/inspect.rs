//! Inspección en vivo de la carpeta de un proyecto. Nada de esto se guarda:
//! se lee cada vez, así el contexto nunca queda desactualizado.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// Máximo de caracteres que se devuelven de cada documento del repo.
const DOC_LIMIT: usize = 6000;
const DOC_FILES: [&str; 3] = ["CLAUDE.md", "AGENTS.md", "README.md"];
const ENV_FILES: [&str; 4] = [".env", ".env.local", ".env.production", ".env.example"];

#[derive(Debug, Serialize)]
pub struct Inspection {
    pub git: Option<GitInfo>,
    pub stack: Option<Stack>,
    pub services: Services,
    pub docs: Vec<Doc>,
}

#[derive(Debug, Serialize)]
pub struct GitInfo {
    pub branch: String,
    /// `dueño/repo` si el remoto es de GitHub.
    pub repo: Option<String>,
    pub uncommitted_files: usize,
    pub recent_commits: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Stack {
    pub package_name: Option<String>,
    pub scripts: Vec<String>,
    pub dependencies: BTreeMap<String, String>,
    pub dev_dependencies: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Services {
    pub vercel_project_id: Option<String>,
    /// Equipo o cuenta de Vercel dueña del proyecto.
    pub vercel_team_id: Option<String>,
    pub supabase_project_ref: Option<String>,
    pub firebase_project: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Doc {
    pub file: String,
    pub content: String,
    pub truncated: bool,
}

/// Servicios de un proyecto: lo detectado en su carpeta, con prioridad para
/// lo que se haya fijado a mano en el registro.
pub fn project_services(project: &crate::Project) -> Services {
    let mut found = project
        .local_path
        .as_deref()
        .map(Path::new)
        .filter(|p| p.is_dir())
        .map(services)
        .unwrap_or_default();
    if project.vercel_project_id.is_some() {
        found.vercel_project_id = project.vercel_project_id.clone();
        found.vercel_team_id = project.vercel_team_id.clone();
    }
    if project.supabase_ref.is_some() {
        found.supabase_project_ref = project.supabase_ref.clone();
    }
    found
}

pub fn inspect(path: &Path) -> Inspection {
    Inspection {
        git: git_info(path, 10),
        stack: stack(path),
        services: services(path),
        docs: docs(path),
    }
}

fn git(path: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(path).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn git_info(path: &Path, commits: usize) -> Option<GitInfo> {
    let branch = git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let log = git(
        path,
        &[
            "log",
            &format!("-{commits}"),
            "--date=short",
            "--pretty=format:%h %ad %s",
        ],
    )
    .unwrap_or_default();
    Some(GitInfo {
        branch,
        repo: git(path, &["remote", "get-url", "origin"]).and_then(|u| github_repo(&u)),
        uncommitted_files: git(path, &["status", "--porcelain"])
            .map(|s| s.lines().count())
            .unwrap_or(0),
        recent_commits: log.lines().map(String::from).collect(),
    })
}

/// `https://github.com/Dueño/Repo.git` o `git@github.com:Dueño/Repo.git` → `Dueño/Repo`.
pub fn github_repo(url: &str) -> Option<String> {
    let rest = url.split_once("github.com")?.1;
    let rest = rest.trim_start_matches([':', '/']);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    (rest.split('/').count() == 2).then(|| rest.to_string())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn string_map(v: &Value) -> BTreeMap<String, String> {
    v.as_object()
        .map(|m| {
            m.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

fn stack(path: &Path) -> Option<Stack> {
    let pkg = read_json(&path.join("package.json"))?;
    Some(Stack {
        package_name: pkg["name"].as_str().map(String::from),
        scripts: string_map(&pkg["scripts"]).into_keys().collect(),
        dependencies: string_map(&pkg["dependencies"]),
        dev_dependencies: string_map(&pkg["devDependencies"]),
    })
}

pub fn services(path: &Path) -> Services {
    let vercel = read_json(&path.join(".vercel/project.json")).unwrap_or_default();
    let vercel_project_id = vercel["projectId"].as_str().map(String::from);
    let vercel_team_id = vercel["orgId"].as_str().map(String::from);
    let firebase_project = read_json(&path.join(".firebaserc"))
        .and_then(|v| v["projects"]["default"].as_str().map(String::from));
    let supabase_project_ref = std::fs::read_to_string(path.join("supabase/.temp/project-ref"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| is_supabase_ref(s))
        .or_else(|| ENV_FILES.iter().find_map(|f| supabase_ref_from_env(&path.join(f))));
    Services {
        vercel_project_id,
        vercel_team_id,
        supabase_project_ref,
        firebase_project,
    }
}

/// Saca solo la referencia del proyecto de la URL pública de Supabase.
/// Nunca devuelve llaves ni ningún otro valor del archivo.
fn supabase_ref_from_env(file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(file).ok()?;
    text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        if !key.contains("SUPABASE_URL") {
            return None;
        }
        let host = value.trim().trim_matches(['"', '\'']).strip_prefix("https://")?;
        let reference = host.strip_suffix(".supabase.co")?;
        is_supabase_ref(reference).then(|| reference.to_string())
    })
}

/// Las referencias de Supabase son 20 letras minúsculas o dígitos. Así se descartan
/// los marcadores de los archivos de ejemplo, como `<ref>` o `tu-proyecto`.
pub fn is_supabase_ref(s: &str) -> bool {
    s.len() == 20 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

fn docs(path: &Path) -> Vec<Doc> {
    DOC_FILES
        .iter()
        .filter_map(|name| {
            let text = std::fs::read_to_string(path.join(name)).ok()?;
            let content: String = text.chars().take(DOC_LIMIT).collect();
            Some(Doc {
                file: name.to_string(),
                truncated: content.len() < text.len(),
                content,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_remotes() {
        assert_eq!(
            github_repo("https://github.com/ana/tienda.git").as_deref(),
            Some("ana/tienda")
        );
        assert_eq!(
            github_repo("git@github.com:ana/tienda-web.git").as_deref(),
            Some("ana/tienda-web")
        );
        assert_eq!(github_repo("https://gitlab.com/a/b.git"), None);
    }

    #[test]
    fn env_parsing_returns_only_the_project_ref() {
        let dir = std::env::temp_dir().join(format!("atalaya-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(".env");
        std::fs::write(
            &file,
            "SUPABASE_SERVICE_ROLE_KEY=secreto\nSUPABASE_URL=https://<ref>.supabase.co\nNEXT_PUBLIC_SUPABASE_URL=\"https://abcdefghij0123456789.supabase.co\"\n",
        )
        .unwrap();
        assert_eq!(supabase_ref_from_env(&file).as_deref(), Some("abcdefghij0123456789"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
