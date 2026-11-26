//! Skills e instrucciones comunes: se instalan una vez en Atalaya y llegan a cada IA cuando
//! trabaja, sin tocar la configuración propia de cada agente.
//!
//! - Una **skill** es una carpeta con un `SKILL.md` (el formato abierto que usan Claude Code,
//!   Codex y otros): un nombre, una descripción y las instrucciones, más los archivos que
//!   necesite. Vive en `skills/<nombre>/`, junto a la base. Se instala apagada: la activas
//!   después de revisar qué trae.
//! - Las **instrucciones** son un texto para todas las IA («usamos pnpm», «no toques las
//!   migraciones») y otro por proyecto.
//!
//! Cómo le llegan a cada una (`for_ai`):
//! - Claude Code con herramientas: las skills como un plugin de esa sesión (`--plugin-dir`),
//!   así las usa como propias; las instrucciones, en el mensaje de sistema.
//! - Los agentes de terminal: la lista de skills con la ruta de cada `SKILL.md`, que su
//!   encierro les deja leer, y las instrucciones.
//! - Lo que no lee archivos (modelos por API, Claude Code sin herramientas): las
//!   instrucciones y el texto de las skills, hasta un tope.

use crate::Registry;
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Lo más grande que se instala: 20 MB y 500 archivos por skill.
const MAX_BYTES: u64 = 20 * 1024 * 1024;
const MAX_FILES: usize = 500;

/// Cuánto texto de skills recibe quien no lee archivos.
const INLINE_BUDGET: usize = 16_000;

/// Un sello único para carpetas temporales y nombres en la Papelera.
fn stamp() -> String {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    format!("{t}-{}", N.fetch_add(1, Ordering::SeqCst))
}

pub fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS skills (
            name TEXT PRIMARY KEY,
            description TEXT NOT NULL DEFAULT '',
            source TEXT NOT NULL DEFAULT '',
            enabled INTEGER NOT NULL DEFAULT 0,
            projects TEXT,
            agents TEXT,
            installed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// De dónde vino: una carpeta o la URL de un repo.
    pub source: String,
    pub enabled: bool,
    /// Solo en estos proyectos; `None` es en todos.
    pub projects: Option<Vec<String>>,
    /// Solo para estas IA (`claude-code` o el id de la conexión); `None` es para todas.
    pub agents: Option<Vec<String>>,
    pub installed_at: String,
    /// Dónde está su `SKILL.md`.
    pub path: String,
}

/// La carpeta de las skills de un registro.
pub fn root_of(registry: &Registry) -> PathBuf {
    registry.path().parent().map(Path::to_path_buf).unwrap_or_default().join("skills")
}

/// La carpeta de las skills de la base de siempre: la que los agentes pueden leer.
pub fn root() -> Result<PathBuf> {
    let db = Registry::default_path()?;
    Ok(db.parent().context("la base no tiene carpeta")?.join("skills"))
}

/// El nombre y la descripción del encabezado de un `SKILL.md` (entre `---`).
pub fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let (mut name, mut description) = (None, None);
    let unquote = |v: &str| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(unquote(v)).filter(|v| !v.is_empty());
        } else if let Some(v) = line.strip_prefix("description:") {
            description = Some(unquote(v)).filter(|v| !v.is_empty());
        }
    }
    (name, description)
}

/// El cuerpo de un `SKILL.md`, sin su encabezado.
fn body(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("---") else { return text };
    match rest.find("\n---") {
        Some(end) => rest[end + 4..].trim_start_matches(['\r', '\n']),
        None => text,
    }
}

/// Un nombre seguro para carpeta: minúsculas, números y guiones.
fn clean_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').chars().take(64).collect::<String>();
    out.trim_end_matches('-').to_string()
}

fn row(r: &rusqlite::Row, root: &Path) -> rusqlite::Result<Skill> {
    let name: String = r.get(0)?;
    let list = |v: Option<String>| v.and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok());
    Ok(Skill {
        path: root.join(&name).join("SKILL.md").to_string_lossy().to_string(),
        name,
        description: r.get(1)?,
        source: r.get(2)?,
        enabled: r.get::<_, i64>(3)? != 0,
        projects: list(r.get(4)?),
        agents: list(r.get(5)?),
        installed_at: r.get(6)?,
    })
}

const COLUMNS: &str = "name, description, source, enabled, projects, agents, installed_at";

pub fn list(registry: &Registry) -> Result<Vec<Skill>> {
    let root = root_of(registry);
    let mut stmt = registry.conn().prepare(&format!("SELECT {COLUMNS} FROM skills ORDER BY name"))?;
    let rows = stmt.query_map([], |r| row(r, &root))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(registry: &Registry, name: &str) -> Result<Option<Skill>> {
    let root = root_of(registry);
    Ok(registry
        .conn()
        .query_row(&format!("SELECT {COLUMNS} FROM skills WHERE name = ?1"), [name], |r| row(r, &root))
        .optional()?)
}

/// Las carpetas con un `SKILL.md` dentro de `dir` (ella misma o hasta tres niveles abajo).
fn find_skills(dir: &Path) -> Vec<PathBuf> {
    if dir.join("SKILL.md").is_file() {
        return vec![dir.to_path_buf()];
    }
    let mut found = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if !path.is_dir() || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            if path.join("SKILL.md").is_file() {
                found.push(path);
            } else if depth < 3 {
                stack.push((path, depth + 1));
            }
        }
    }
    found.sort();
    found
}

/// Copia una carpeta sin seguir enlaces ni llevar `.git`.
fn copy_dir(from: &Path, to: &Path, count: &mut (usize, u64)) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        if name == ".git" {
            continue;
        }
        let kind = e.file_type()?;
        let target = to.join(&name);
        if kind.is_dir() {
            copy_dir(&e.path(), &target, count)?;
        } else if kind.is_file() {
            count.0 += 1;
            count.1 += e.metadata()?.len();
            if count.0 > MAX_FILES || count.1 > MAX_BYTES {
                bail!("la skill es demasiado grande (más de {MAX_FILES} archivos o {} MB)", MAX_BYTES / 1024 / 1024);
            }
            std::fs::copy(e.path(), &target)?;
        }
        // Los enlaces simbólicos no se copian: podrían apuntar fuera de la skill.
    }
    Ok(())
}

/// Instala las skills que haya en una carpeta. Quedan apagadas hasta que las revises.
pub fn install_folder(registry: &Registry, dir: &Path, source: &str) -> Result<Vec<Skill>> {
    let dirs = find_skills(dir);
    if dirs.is_empty() {
        bail!("no hay ninguna skill en {}: cada skill es una carpeta con un SKILL.md", dir.display());
    }
    let root = root_of(registry);
    std::fs::create_dir_all(&root)?;
    let mut installed = Vec::new();
    for d in dirs {
        let text = std::fs::read_to_string(d.join("SKILL.md"))?;
        let (name, description) = front_matter(&text);
        let folder_name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let name = clean_name(name.as_deref().unwrap_or(&folder_name));
        if name.is_empty() {
            bail!("la skill de {} no tiene un nombre válido", d.display());
        }
        if get(registry, &name)?.is_some() || root.join(&name).exists() {
            bail!("ya hay una skill llamada «{name}»: quítala antes de instalarla de nuevo");
        }
        let target = root.join(&name);
        if let Err(e) = copy_dir(&d, &target, &mut (0, 0)) {
            // Lo copiado a medias es nuestro y está incompleto.
            let _ = std::fs::remove_dir_all(&target);
            return Err(e);
        }
        registry.conn().execute(
            "INSERT INTO skills (name, description, source) VALUES (?1, ?2, ?3)",
            params![name, description.unwrap_or_default(), source],
        )?;
        installed.push(get(registry, &name)?.expect("recién instalada"));
    }
    Ok(installed)
}

/// Instala desde un repo de git: lo clona sin historia en una carpeta temporal.
pub fn install_git(registry: &Registry, url: &str) -> Result<Vec<Skill>> {
    let url = url.trim();
    let ok = url.starts_with("https://") || url.starts_with("git@") || url.starts_with("ssh://");
    if !ok || url.chars().any(char::is_whitespace) {
        bail!("usa la URL del repo (https://… o git@…)");
    }
    let tmp = std::env::temp_dir().join(format!("atalaya-skill-{}-{}", std::process::id(), stamp()));
    let out = Command::new("git")
        .args(["clone", "--depth", "1", "--quiet", "--", url])
        .arg(&tmp)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("PATH", crate::worker::developer_path())
        .output()
        .context("no se pudo ejecutar git")?;
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("no se pudo clonar: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let result = install_folder(registry, &tmp, url);
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Las skills que ya tienes en tus agentes (fuera de Atalaya) y aún no instalaste aquí.
#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub name: String,
    pub description: String,
    pub path: String,
    /// Dónde la encontró: «Claude Code» o «Codex y otros».
    pub origin: String,
}

pub fn found_elsewhere(registry: &Registry) -> Result<Vec<Found>> {
    let home = dirs::home_dir().unwrap_or_default();
    let installed: Vec<String> = list(registry)?.into_iter().map(|s| s.name).collect();
    let mut found = Vec::new();
    for (rel, origin) in [(".claude/skills", "Claude Code"), (".agents/skills", "Codex y otros")] {
        for d in find_skills(&home.join(rel)) {
            let text = std::fs::read_to_string(d.join("SKILL.md")).unwrap_or_default();
            let (name, description) = front_matter(&text);
            let folder_name = d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let name = clean_name(name.as_deref().unwrap_or(&folder_name));
            if name.is_empty() || installed.contains(&name) || found.iter().any(|f: &Found| f.name == name) {
                continue;
            }
            found.push(Found {
                name,
                description: description.unwrap_or_default(),
                path: d.to_string_lossy().to_string(),
                origin: origin.into(),
            });
        }
    }
    Ok(found)
}

/// Qué trae una skill, para revisarla antes de activarla.
#[derive(Debug, Clone, Serialize)]
pub struct Review {
    pub skill: Skill,
    pub text: String,
    pub files: Vec<FileInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub path: String,
    pub size: u64,
    /// Un programa que una IA podría ejecutar.
    pub script: bool,
}

fn is_script(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let executable = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false);
    executable || matches!(ext.as_str(), "sh" | "bash" | "zsh" | "py" | "js" | "mjs" | "cjs" | "ts" | "rb" | "pl" | "ps1")
}

pub fn review(registry: &Registry, name: &str) -> Result<Review> {
    let skill = get(registry, name)?.with_context(|| format!("no hay una skill «{name}»"))?;
    let dir = root_of(registry).join(name);
    let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
    let mut files = Vec::new();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(&dir).unwrap_or(&path).to_string_lossy().to_string();
                files.push(FileInfo { size: e.metadata().map(|m| m.len()).unwrap_or(0), script: is_script(&path), path: rel });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Review { skill, text, files })
}

/// Activa o apaga una skill, y dónde rige.
pub fn configure(
    registry: &Registry,
    name: &str,
    enabled: bool,
    projects: Option<Vec<String>>,
    agents: Option<Vec<String>>,
) -> Result<Skill> {
    let json = |v: Option<Vec<String>>| v.map(|l| serde_json::to_string(&l).unwrap_or_default());
    let n = registry.conn().execute(
        "UPDATE skills SET enabled = ?2, projects = ?3, agents = ?4 WHERE name = ?1",
        params![name, enabled as i64, json(projects), json(agents)],
    )?;
    if n == 0 {
        bail!("no hay una skill «{name}»");
    }
    Ok(get(registry, name)?.expect("existe"))
}

/// Quita una skill: su carpeta va a la Papelera, no se borra.
pub fn remove(registry: &Registry, name: &str) -> Result<()> {
    get(registry, name)?.with_context(|| format!("no hay una skill «{name}»"))?;
    let dir = root_of(registry).join(name);
    if dir.exists() {
        let trash = dirs::home_dir().context("sin carpeta de usuario")?.join(".Trash");
        std::fs::create_dir_all(&trash)?;
        let target = trash.join(format!("skill-{name}-{}", stamp()));
        std::fs::rename(&dir, &target).with_context(|| format!("no se pudo mover {} a la Papelera", dir.display()))?;
    }
    registry.conn().execute("DELETE FROM skills WHERE name = ?1", [name])?;
    Ok(())
}

/* ------------------------------------------------------------------ Instrucciones */

fn instructions_key(slug: Option<&str>) -> String {
    match slug {
        Some(s) => format!("instrucciones:{s}"),
        None => "instrucciones".into(),
    }
}

pub fn instructions(registry: &Registry, slug: Option<&str>) -> Result<String> {
    Ok(registry.setting(&instructions_key(slug))?.unwrap_or_default())
}

pub fn set_instructions(registry: &Registry, slug: Option<&str>, text: &str) -> Result<()> {
    registry.set_setting(&instructions_key(slug), text.trim())
}

/* ------------------------------------------------------------------ Entrega */

/// Cómo lee la IA que va a recibirlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    /// Claude Code con herramientas: las skills van como plugin.
    Plugin,
    /// Lee archivos por su cuenta: recibe la ruta de cada skill.
    Files,
    /// No lee archivos: recibe el texto.
    Inline,
}

/// Las skills activas para este proyecto y esta IA.
pub fn active(registry: &Registry, slug: Option<&str>, ai: &str) -> Result<Vec<Skill>> {
    Ok(list(registry)?
        .into_iter()
        .filter(|s| s.enabled)
        .filter(|s| match (&s.projects, slug) {
            (None, _) => true,
            (Some(list), Some(slug)) => list.iter().any(|p| p == slug),
            // Sin proyecto en foco, una skill de ciertos proyectos no aplica.
            (Some(_), None) => false,
        })
        .filter(|s| s.agents.as_ref().is_none_or(|list| list.iter().any(|a| a == ai)))
        .collect())
}

/// Lo que se agrega al mensaje de sistema (o al encargo) de una IA.
pub fn for_ai(registry: &Registry, slug: Option<&str>, ai: &str, reader: Reader) -> Result<String> {
    let mut out = String::new();
    let general = instructions(registry, None)?;
    if !general.is_empty() {
        out.push_str(&format!("\n\nInstrucciones del usuario para todas las IA:\n{general}"));
    }
    if let Some(slug) = slug {
        let own = instructions(registry, Some(slug))?;
        if !own.is_empty() {
            out.push_str(&format!("\n\nInstrucciones del usuario para el proyecto «{slug}»:\n{own}"));
        }
    }
    out.push_str(&crate::memories::for_ai(registry, slug)?);
    let skills = active(registry, slug, ai)?;
    if skills.is_empty() || reader == Reader::Plugin {
        return Ok(out);
    }
    match reader {
        Reader::Files => {
            out.push_str(
                "\n\nSkills instaladas por el usuario. Cuando una sirva para lo que haces, lee primero su SKILL.md y sigue sus instrucciones (sus otros archivos están en la misma carpeta):",
            );
            for s in &skills {
                out.push_str(&format!("\n- {}: {} ({})", s.name, s.description, s.path));
            }
        }
        Reader::Inline => {
            out.push_str("\n\nSkills del usuario. Úsalas cuando sirvan para lo que haces:");
            let mut left = INLINE_BUDGET;
            for s in &skills {
                let text = std::fs::read_to_string(&s.path).unwrap_or_default();
                let text = body(&text).trim();
                if left < 200 {
                    out.push_str(&format!("\n\n### {}\n{} (sin espacio para su texto)", s.name, s.description));
                    continue;
                }
                let clipped: String = text.chars().take(left).collect();
                left = left.saturating_sub(clipped.chars().count());
                out.push_str(&format!("\n\n### {}\n{}\n\n{clipped}", s.name, s.description));
            }
        }
        Reader::Plugin => {}
    }
    Ok(out)
}

/// Para Claude Code: un plugin de esa sesión con las skills activas, en una carpeta
/// temporal (su encierro no lee los datos de Atalaya). `None` si no hay ninguna.
pub fn plugin_dir(registry: &Registry, slug: Option<&str>, ai: &str) -> Result<Option<PathBuf>> {
    let skills = active(registry, slug, ai)?;
    if skills.is_empty() {
        return Ok(None);
    }
    let parent = std::env::temp_dir().join("atalaya-skills");
    // Las copias de sesiones de hace más de un día ya no sirven: son de Atalaya, no tuyas.
    if let Ok(old) = std::fs::read_dir(&parent) {
        for e in old.flatten() {
            let stale = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            if stale.is_some_and(|age| age > std::time::Duration::from_secs(24 * 3600)) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let base = parent.join(format!("{}-{}", std::process::id(), stamp()));
    std::fs::create_dir_all(base.join(".claude-plugin"))?;
    std::fs::write(
        base.join(".claude-plugin").join("plugin.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "atalaya",
            "description": "Las skills que instalaste en Atalaya",
            "version": env!("CARGO_PKG_VERSION"),
        }))?,
    )?;
    let root = root_of(registry);
    for s in &skills {
        copy_dir(&root.join(&s.name), &base.join("skills").join(&s.name), &mut (0, 0))?;
    }
    Ok(Some(base))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil;

    fn skill_dir(parent: &Path, folder: &str, front: &str, body: &str) -> PathBuf {
        let d = parent.join(folder);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("SKILL.md"), format!("---\n{front}\n---\n\n{body}\n")).unwrap();
        d
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("atalaya-skills-{name}-{}-{}", std::process::id(), stamp()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_the_header_of_a_skill() {
        let (n, d) = front_matter("---\nname: revisar-pr\ndescription: \"Revisa un PR: riesgos y pruebas\"\n---\n# Hola");
        assert_eq!(n.as_deref(), Some("revisar-pr"));
        assert_eq!(d.as_deref(), Some("Revisa un PR: riesgos y pruebas"));
        assert_eq!(front_matter("# Sin encabezado"), (None, None));
        assert_eq!(body("---\nname: x\n---\n\n# Cuerpo"), "# Cuerpo");
        assert_eq!(clean_name("Mi Skill_de PRUEBA!"), "mi-skill-de-prueba");
    }

    #[test]
    fn installs_off_reviews_and_reaches_each_ai_its_own_way() {
        let r = testutil::registry("skills");
        testutil::project(&r, "web");
        testutil::project(&r, "api");
        let src = temp("fuente");
        let a = skill_dir(&src, "estilo", "name: estilo-web\ndescription: Cómo escribir CSS aquí", "Usa tokens, nunca colores sueltos.");
        std::fs::write(a.join("revisar.sh"), "echo hola").unwrap();
        skill_dir(&src.join("mas"), "pruebas", "description: Cómo correr las pruebas", "Corre pnpm test antes de terminar.");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", a.join("enlace")).unwrap();

        let installed = install_folder(&r, &src, "carpeta de prueba").unwrap();
        let names: Vec<&str> = installed.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["estilo-web", "pruebas"]);
        assert!(installed.iter().all(|s| !s.enabled), "se instalan apagadas");
        assert!(install_folder(&r, &a, "otra vez").is_err(), "no pisa una instalada");

        let rev = review(&r, "estilo-web").unwrap();
        let files: Vec<(&str, bool)> = rev.files.iter().map(|f| (f.path.as_str(), f.script)).collect();
        assert_eq!(files, vec![("SKILL.md", false), ("revisar.sh", true)], "sin el enlace");
        assert!(rev.text.contains("Usa tokens"));

        // Apagadas, no llegan a nadie.
        assert!(for_ai(&r, Some("web"), "claude-code", Reader::Files).unwrap().is_empty());

        configure(&r, "estilo-web", true, Some(vec!["web".into()]), None).unwrap();
        configure(&r, "pruebas", true, None, Some(vec!["codex-1".into()])).unwrap();
        set_instructions(&r, None, "Responde en español.").unwrap();
        set_instructions(&r, Some("web"), "Usamos pnpm.").unwrap();

        let files = for_ai(&r, Some("web"), "codex-1", Reader::Files).unwrap();
        assert!(files.contains("Responde en español.") && files.contains("Usamos pnpm."));
        assert!(files.contains("estilo-web: Cómo escribir CSS aquí") && files.contains("pruebas:"));
        assert!(files.contains(&root_of(&r).join("estilo-web").join("SKILL.md").to_string_lossy().to_string()));

        // En otro proyecto no va la de «web», ni sus instrucciones.
        let api = for_ai(&r, Some("api"), "codex-1", Reader::Files).unwrap();
        assert!(!api.contains("estilo-web") && !api.contains("pnpm") && api.contains("pruebas"));

        // Otra IA no recibe la que es solo para codex-1.
        let inline = for_ai(&r, Some("web"), "anthropic-1", Reader::Inline).unwrap();
        assert!(inline.contains("Usa tokens, nunca colores sueltos.") && !inline.contains("pnpm test"));

        // Claude Code: las instrucciones en el texto y las skills en su plugin.
        let text = for_ai(&r, Some("web"), "claude-code", Reader::Plugin).unwrap();
        assert!(text.contains("Usamos pnpm.") && !text.contains("estilo-web"));
        let plugin = plugin_dir(&r, Some("web"), "claude-code").unwrap().unwrap();
        assert!(plugin.join(".claude-plugin/plugin.json").is_file());
        assert!(plugin.join("skills/estilo-web/SKILL.md").is_file());
        assert!(plugin.join("skills/estilo-web/revisar.sh").is_file());
        assert!(!plugin.join("skills/pruebas").exists());
        assert!(plugin_dir(&r, Some("api"), "claude-code").unwrap().is_none());
    }

    #[test]
    fn a_terminal_agent_reads_the_skill_from_its_sandbox() {
        use crate::connections::{Input, save};
        use crate::orchestrator::{Candidate, ask};
        let r = testutil::registry("skills-agente");
        let src = temp("fuente-agente");
        skill_dir(&src, "clave", "name: clave\ndescription: La palabra clave", "La palabra es GIRASOL-77.");
        install_folder(&r, &src, "prueba").unwrap();
        configure(&r, "clave", true, None, None).unwrap();
        set_instructions(&r, None, "Firma con «— prueba».").unwrap();
        let script = src.join("agente.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nruta=$(printf '%s' \"$1\" | grep -o '([^()]*SKILL.md)' | head -1 | tr -d '()')\necho \"Dice: $(tail -n 1 \"$ruta\")\"\nprintf '%s' \"$1\" | grep -o 'Firma con[^.]*'\n",
        )
        .unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
        let command = format!("{} {{tarea}}", script.display());
        let c = save(
            &r,
            &Input {
                id: None,
                provider: "agente-personalizado".into(),
                name: "Falso".into(),
                config: serde_json::json!({ "command": command, "ask_command": command }),
                secret: None,
            },
        )
        .unwrap();
        let reply = ask(&r, &Candidate::Connection(c), "Eres una prueba.", "¿Cuál es la palabra?", "prueba", None).unwrap();
        assert!(reply.contains("Dice: La palabra es GIRASOL-77."), "{reply}");
        assert!(reply.contains("Firma con «— prueba»"), "{reply}");
    }

    #[test]
    fn rejects_folders_without_skills() {
        let r = testutil::registry("skills-vacio");
        let empty = temp("vacia");
        assert!(install_folder(&r, &empty, "x").is_err());
        assert!(install_git(&r, "file:///etc").is_err());
        assert!(install_git(&r, "https://x y").is_err());
    }
}
