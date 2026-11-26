//! Poner al día la carpeta local de un proyecto con su rama principal.
//!
//! Tras fusionar un cambio, Atalaya trae lo nuevo a la carpeta del usuario para que las
//! revisiones vean el código real. Solo avanza «hacia adelante» (fast-forward) y deja que
//! git decida qué es seguro: los archivos nuevos o modificados que no chocan con el cambio
//! se quedan como estaban; si algo se pisaría, git no toca nada.
//!
//! Cuando no se puede (otra rama, commits propios o archivos que chocan), el proyecto queda
//! anotado y el Vigía lo reintenta en cada vuelta. Mientras tanto hay una decisión
//! «Poner al día» que ejecuta Atalaya mismo: los agentes trabajan en una copia aparte y no
//! pueden tocar la carpeta del usuario, así que nunca se les manda este trabajo.

use crate::{Project, Registry};
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

/// Proyectos cuya carpeta quedó atrasada y se reintentan (JSON con sus slugs).
const PENDING: &str = "local_pendiente";

/// La acción de decisión que pone al día la carpeta.
pub const ACTION: &str = "actualizar";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Avanzó con el cambio.
    Done,
    /// Ya estaba al día.
    Already,
    /// No se pudo con seguridad; se reintenta más tarde.
    Blocked,
    /// No hay carpeta con git en esta máquina.
    NoFolder,
}

#[derive(Debug, Clone)]
pub struct Sync {
    pub outcome: Outcome,
    pub text: String,
}

fn sync(outcome: Outcome, text: impl Into<String>) -> Sync {
    Sync { outcome, text: text.into() }
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn default_branch(repo: &Path) -> String {
    git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|s| s.strip_prefix("origin/").map(String::from))
        .unwrap_or_else(|| "main".into())
}

/// Los archivos sin confirmar, legibles: «supabase/.temp/ (nuevo)», «src/a.ts (modificado)».
pub fn pending_files(repo: &Path) -> Vec<String> {
    // Sin recortar: la primera columna del formato puede ser un espacio (« M archivo»).
    let out = Command::new("git").arg("-C").arg(repo).args(["status", "--porcelain", "--untracked-files=normal"]).output();
    let text = out.ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    text.unwrap_or_default()
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let (code, path) = l.split_at(3);
            let kind = match code.trim() {
                "??" => "nuevo",
                "D" => "borrado",
                c if c.contains('R') => "renombrado",
                _ => "modificado",
            };
            format!("{} ({kind})", path.trim().trim_matches('"'))
        })
        .collect()
}

/// Hasta tres nombres y «y N más».
pub fn short_list(files: &[String]) -> String {
    let mut text = files.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if files.len() > 3 {
        text.push_str(&format!(" y {} más", files.len() - 3));
    }
    text
}

/// Trae a la carpeta local lo último de la rama principal, solo si es seguro.
/// `branch` es la rama de trabajo ya fusionada, que deja de hacer falta aquí.
pub fn update(project: &Project, branch: Option<&str>) -> Sync {
    let Some(repo) = project.local_path.as_deref().map(Path::new).filter(|p| p.join(".git").exists()) else {
        return sync(Outcome::NoFolder, "Sin carpeta local que actualizar.");
    };
    if let Some(branch) = branch {
        let _ = git(repo, &["branch", "-D", branch]);
    }
    let base = default_branch(repo);
    if git(repo, &["fetch", "origin", &base]).is_err() {
        return sync(Outcome::Blocked, "No se pudo traer el cambio del remoto; la carpeta local sigue igual.");
    }
    let current = git(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    if current != base {
        return sync(
            Outcome::Blocked,
            format!("Tu carpeta está en la rama {current}, así que no la toqué; el cambio está en origin/{base}."),
        );
    }
    let count = |range: String| git(repo, &["rev-list", "--count", &range]).ok().and_then(|n| n.parse::<u64>().ok());
    let behind = count(format!("HEAD..origin/{base}")).unwrap_or(1);
    if behind == 0 {
        return sync(Outcome::Already, "Tu carpeta local ya estaba al día.");
    }
    if count(format!("origin/{base}..HEAD")).unwrap_or(0) > 0 {
        return sync(
            Outcome::Blocked,
            format!("Tu carpeta tiene commits propios, así que no la toqué; el cambio está en origin/{base}."),
        );
    }
    // git solo avanza si no pisa nada: lo que no choca con el cambio se queda como estaba.
    let files = pending_files(repo);
    match git(repo, &["merge", "--ff-only", "--quiet", &format!("origin/{base}")]) {
        Ok(_) => {
            let kept = if files.is_empty() {
                String::new()
            } else {
                format!(" Tus archivos sin confirmar siguen como estaban: {}.", short_list(&files))
            };
            sync(
                Outcome::Done,
                format!("Tu carpeta local quedó al día con el cambio.{kept} Si cambiaron dependencias, ejecuta npm install."),
            )
        }
        Err(_) if !files.is_empty() => sync(
            Outcome::Blocked,
            format!(
                "Tu carpeta tiene cambios sin confirmar que chocan con lo nuevo ({}), así que no la toqué; el cambio \
                 está en origin/{base}.",
                short_list(&files)
            ),
        ),
        Err(e) => sync(Outcome::Blocked, format!("No se pudo avanzar la carpeta local ({e:#}); el cambio está en origin/{base}.")),
    }
}

fn pending(registry: &Registry) -> Vec<String> {
    registry
        .setting(PENDING)
        .ok()
        .flatten()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

fn set_pending(registry: &Registry, slugs: &[String]) -> Result<()> {
    registry.set_setting(PENDING, &serde_json::to_string(slugs)?)
}

/// Pone al día y lleva la cuenta: si queda bloqueada, se anota para reintentar y deja una
/// decisión «Poner al día»; si se resolvió, la saca de la lista y cierra esa decisión.
pub fn track(registry: &Registry, project: &Project, branch: Option<&str>) -> Sync {
    let result = update(project, branch);
    if let Err(e) = remember(registry, project, &result) {
        eprintln!("no se pudo anotar el estado de la carpeta de {}: {e:#}", project.slug);
    }
    result
}

fn remember(registry: &Registry, project: &Project, result: &Sync) -> Result<()> {
    let mut slugs = pending(registry);
    let listed = slugs.contains(&project.slug);
    match result.outcome {
        Outcome::Blocked => {
            if !listed {
                slugs.push(project.slug.clone());
                set_pending(registry, &slugs)?;
            }
            let title = format!("Poner al día la carpeta de {}", project.name);
            let proposal = "Atalaya vuelve a intentar traer el cambio a tu carpeta local. Si sigue sin poder, deja \
                            la carpeta en la rama principal y confirma, guarda o descarta lo que choca.";
            // Si ya hay una esperando, no se repite.
            let _ = crate::decisions::propose(registry, &project.slug, "carpeta", &title, &result.text, proposal, ACTION, None);
        }
        Outcome::Done | Outcome::Already | Outcome::NoFolder => {
            if listed {
                slugs.retain(|s| s != &project.slug);
                set_pending(registry, &slugs)?;
            }
            crate::decisions::settle(registry, &project.slug, ACTION, &result.text)?;
        }
    }
    Ok(())
}

/// El Vigía reintenta las carpetas que quedaron atrasadas. Devuelve cuántas avanzaron.
pub fn retry(registry: &Registry) -> Result<usize> {
    let mut done = 0;
    for slug in pending(registry) {
        let Some(project) = registry.get(&slug)? else {
            let rest: Vec<String> = pending(registry).into_iter().filter(|s| s != &slug).collect();
            set_pending(registry, &rest)?;
            continue;
        };
        let result = track(registry, &project, None);
        if result.outcome == Outcome::Done {
            registry.record_event(&slug, &result.text, "vigía")?;
            done += 1;
        }
    }
    Ok(done)
}

/// La acción de la decisión «Poner al día»: si sigue sin poder, falla con el motivo y la
/// decisión queda pendiente.
pub fn now(registry: &Registry, slug: &str) -> Result<String> {
    let project = registry.get(slug)?.context("el proyecto ya no existe")?;
    let result = update(&project, None);
    if result.outcome == Outcome::Blocked {
        bail!("{}", result.text);
    }
    let mut slugs = pending(registry);
    if slugs.contains(&project.slug) {
        slugs.retain(|s| s != &project.slug);
        set_pending(registry, &slugs)?;
    }
    Ok(result.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, script: &str) {
        let ok = Command::new("sh").arg("-c").arg(script).current_dir(dir).output().unwrap().status.success();
        assert!(ok, "falló: {script}");
    }

    fn project(path: &Path) -> Project {
        Project {
            slug: "demo".into(),
            name: "Demo".into(),
            description: None,
            local_path: Some(path.to_string_lossy().into()),
            repo: None,
            production_url: None,
            notes: None,
            vercel_project_id: None,
            vercel_team_id: None,
            supabase_ref: None,
            updated_at: String::new(),
        }
    }

    /// Un remoto, la carpeta del usuario (`a`) y otra que empuja cambios (`b`).
    fn setup(name: &str) -> (std::path::PathBuf, impl Fn(&str)) {
        let root = std::env::temp_dir().join(format!("atalaya-local-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let id = "-c user.name=P -c user.email=p@example.invalid";
        sh(&root, "git init -q --bare remoto.git && git clone -q remoto.git a 2>/dev/null");
        sh(
            &root.join("a"),
            &format!("git checkout -q -b main && echo 1 > f && git add f && git {id} commit -q -m uno && git push -q -u origin main && git remote set-head origin main"),
        );
        sh(&root, "git clone -q remoto.git b");
        let b = root.join("b");
        let push = move |script: &str| sh(&b, &format!("git pull -q && {script} && git add -A && git {id} commit -qm cambio && git push -q"));
        (root, push)
    }

    #[test]
    fn advances_unless_something_would_be_lost() {
        let (root, push) = setup("ff");
        let a = root.join("a");
        let read = |f: &str| std::fs::read_to_string(a.join(f)).unwrap().trim().to_string();

        push("echo 2 > f");
        assert_eq!(update(&project(&a), None).outcome, Outcome::Done);
        assert_eq!(read("f"), "2");
        assert_eq!(update(&project(&a), None).outcome, Outcome::Already);

        // Un archivo nuevo (como una caché que el cambio pasa a ignorar) ya no bloquea.
        std::fs::create_dir_all(a.join("supabase/.temp")).unwrap();
        std::fs::write(a.join("supabase/.temp/ref"), "x").unwrap();
        push("echo 'supabase/.temp/' > .gitignore");
        let r = update(&project(&a), None);
        assert_eq!(r.outcome, Outcome::Done, "{}", r.text);
        assert!(git(&a, &["status", "--porcelain"]).unwrap().is_empty(), "la caché queda ignorada");
        assert_eq!(read("supabase/.temp/ref"), "x", "y en su lugar");

        // Un cambio local que no choca se conserva.
        std::fs::write(a.join("notas"), "mías").unwrap();
        push("echo 3 > f");
        let r = update(&project(&a), None);
        assert_eq!(r.outcome, Outcome::Done);
        assert!(r.text.contains("notas (nuevo)"), "{}", r.text);
        assert_eq!(read("notas"), "mías");

        // Si chocan, no se toca nada.
        std::fs::write(a.join("f"), "local").unwrap();
        push("echo 4 > f");
        let r = update(&project(&a), None);
        assert_eq!(r.outcome, Outcome::Blocked);
        assert!(r.text.contains("chocan") && r.text.contains("f (modificado)"), "{}", r.text);
        assert_eq!(read("f"), "local");
        sh(&a, "git checkout -q -- f");

        // En otra rama tampoco.
        sh(&a, "git checkout -q -b otra");
        assert!(update(&project(&a), None).text.contains("rama otra"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Bloqueada deja una decisión y queda para reintentar; al resolverse, se cierra sola.
    #[test]
    fn blocked_folder_is_retried_and_its_decision_closes() {
        let (root, push) = setup("retry");
        let a = root.join("a");
        let r = crate::testutil::registry("local-retry");
        r.upsert(&project(&a)).unwrap();

        sh(&a, "git checkout -q -b revisando");
        push("echo 2 > f");
        assert_eq!(track(&r, &project(&a), None).outcome, Outcome::Blocked);
        assert_eq!(pending(&r), vec!["demo".to_string()]);
        let open = crate::decisions::list(&r, true).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].action, ACTION);
        track(&r, &project(&a), None);
        assert_eq!(crate::decisions::list(&r, true).unwrap().len(), 1, "no se repite");
        assert!(crate::decisions::approve(&r, open[0].id, "", false).is_err(), "sigue sin poder: queda pendiente");

        sh(&a, "git checkout -q main");
        assert_eq!(retry(&r).unwrap(), 1);
        assert!(pending(&r).is_empty());
        assert!(crate::decisions::list(&r, true).unwrap().is_empty(), "la decisión se cerró sola");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
