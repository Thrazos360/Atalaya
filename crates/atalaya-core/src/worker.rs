//! El trabajador: toma una mejora aceptada y la lleva hasta un pull request.
//!
//! Claude Code trabaja solo, pero dentro de límites fijos:
//! - en una copia aparte del repo (un worktree) y en una rama nueva, nunca en la
//!   carpeta del usuario ni en la rama principal;
//! - puede editar archivos, instalar dependencias, probar y hacer commits, pero no
//!   puede subir nada ni desplegar: eso lo hace Atalaya después, y solo a la rama;
//! - el cambio llega a producción únicamente cuando una persona fusiona el pull request.

use crate::orchestrator::{CLAUDE_CODE, Candidate};
use crate::{Improvement, Project, Registry, github};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// El agente puede usar todas sus herramientas, incluida cualquier orden de terminal:
/// instalar, compilar, probar, buscar documentación. Es lo que necesita para resolver
/// una mejora completa sin preguntar a cada paso.
const ALLOWED: &str = "Read Edit Write Grep Glob Bash WebFetch WebSearch Skill";

/// Lo único vedado es lo que saca el trabajo de la copia: subir, desplegar o tocar
/// servicios en producción. Eso espera el visto bueno de una persona.
const DENIED: &str = "Bash(git push *) Bash(git remote *) Bash(gh *) Bash(vercel*) Bash(npx vercel*) \
Bash(firebase deploy*) Bash(npx firebase*) Bash(supabase db push*) Bash(npx supabase db push*) Bash(sudo *)";

/// Aislamiento a nivel del sistema para las órdenes de terminal del agente: solo puede
/// escribir en su copia de trabajo y en las cachés que necesitan las herramientas, no
/// puede leer llaves ni credenciales, solo sale a internet hacia los registros de
/// paquetes y GitHub, y no hay forma de saltarse el aislamiento.
/// Las herramientas de edición quedan limitadas a la carpeta de trabajo.
const SANDBOX: &str = r#"{
  "sandbox": {
    "enabled": true,
    "autoAllowBashIfSandboxed": true,
    "allowUnsandboxedCommands": false,
    "filesystem": {
      "allowWrite": ["~/.npm", "~/.cache", "~/Library/Caches", "~/.pnpm-store", "~/.yarn"],
      "denyRead": ["~/.ssh", "~/.aws", "~/.gnupg", "~/.config/gh", "~/.config/configstore", "~/Library/Keychains", "~/Library/Application Support/Atalaya", "~/Library/Application Support/com.vercel.cli"]
    },
    "network": {
      "allowedDomains": [
        "registry.npmjs.org", "*.npmjs.org", "registry.yarnpkg.com", "nodejs.org", "*.nodejs.org",
        "github.com", "*.github.com", "*.githubusercontent.com",
        "fonts.googleapis.com", "fonts.gstatic.com", "cdn.jsdelivr.net", "unpkg.com", "jsr.io",
        "pypi.org", "files.pythonhosted.org", "crates.io", "*.crates.io"
      ]
    }
  },
  "permissions": { "blockWritesOutsideWorkingDirectories": true }
}"#;

const RULES: &str = "Trabajas de forma autónoma para Atalaya en una copia aparte del repositorio, en una rama propia. \
Tienes libertad para hacer todo lo necesario dentro de esta carpeta: leer y editar código, instalar y actualizar \
dependencias, ejecutar pruebas, lint y build, y buscar documentación en la web. No pidas permiso ni dejes el \
trabajo a medias: resuélvelo completo. Límites: \
1) Todo ocurre dentro de esta carpeta; no modifiques nada fuera de ella. \
2) No subas, no despliegues y no cambies nada en servicios en producción (git push, Vercel, Firebase, Supabase): \
eso lo decide el usuario al revisar tu trabajo. \
3) No toques archivos .env ni secretos. \
4) Verifica tu trabajo: si hay pruebas, lint o build, ejecútalos y deja todo pasando; si algo ya fallaba antes \
de tus cambios, dilo. \
5) Confirma el trabajo con uno o más commits en español. \
6) Atalaya lleva el estado de la mejora y abre el pull request: no intentes hacerlo tú. \
7) Si una parte no se puede hacer con seguridad, haz el resto y explica qué quedó fuera y por qué. \
8) Termina con un informe en español, en texto simple, para alguien que no va a leer el código: qué problema \
había, qué solución aplicaste, cómo la verificaste y qué debería revisar antes de aprobar. \
9) Nadie te responde mientras trabajas. Si te falta un detalle menor, elige la opción más razonable y dilo en el \
informe. Solo si de verdad no puedes avanzar sin una decisión del usuario, no hagas cambios a medias: termina con \
la pregunta, clara y concreta, en una línea que empiece con «Pregunta:». El usuario te responde desde Atalaya y \
vuelves a empezar con su respuesta. \
10) Si al revisar concluyes que no hace falta cambiar el código (ya está resuelto, o la causa está fuera del \
repositorio: la carpeta local del usuario, que Atalaya pone al día sola, o un servicio en producción), no hagas \
commits: termina con una línea que empiece con «Sin cambios:» y el motivo en una frase, y en el informe explica qué \
tendría que hacer el usuario, si algo. Así Atalaya cierra la mejora sin pasársela a otro agente.";

/// El PATH de una terminal del usuario. Una app abierta desde Finder o un servicio
/// arrancan con un PATH mínimo, sin Node, npm ni las herramientas de Homebrew o nvm.
pub fn developer_path() -> String {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        // Interactiva y de inicio de sesión: así carga también lo que nvm añade en .zshrc.
        let from_shell = Command::new(&shell)
            .args(["-ilc", "printf '\\n%s' \"$PATH\""])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .and_then(|out| String::from_utf8_lossy(&out.stdout).lines().last().map(String::from))
            .unwrap_or_default();
        let current = std::env::var("PATH").unwrap_or_default();
        let mut seen = std::collections::HashSet::new();
        [from_shell.as_str(), current.as_str(), "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"]
            .iter()
            .flat_map(|list| list.split(':'))
            .filter(|dir| !dir.is_empty() && seen.insert(dir.to_string()))
            .collect::<Vec<_>>()
            .join(":")
    })
    .clone()
}

/// Procesos del agente en marcha, para poder detenerlos si Atalaya se cierra.
static RUNNING: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// Detiene a los agentes en marcha. Se llama al salir de Atalaya: sin esto seguirían
/// trabajando sin que nadie recoja su resultado.
pub fn stop_all() {
    for pid in RUNNING.lock().unwrap().drain(..) {
        let _ = Command::new("kill").arg(pid.to_string()).output();
    }
}

/// Busca el comando `claude`: una app abierta desde Finder o un servicio no heredan el PATH de la terminal.
pub fn find_claude() -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    from_path
        .into_iter()
        .chain(dirs::home_dir().map(|h| h.join(".local/bin")))
        .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from))
        .map(|dir| dir.join("claude"))
        .find(|p| p.is_file())
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Carpeta donde viven las copias de trabajo: fuera de los proyectos del usuario y
/// también fuera de la carpeta de datos de Atalaya, que el agente tiene vedada
/// (ahí están la base de datos y los respaldos).
pub fn worktrees_dir() -> Result<PathBuf> {
    let base = Registry::default_path()?;
    let data = base.parent().context("sin carpeta de datos")?;
    let name = format!("{}-trabajo", data.file_name().and_then(|n| n.to_str()).unwrap_or("Atalaya"));
    Ok(data.with_file_name(name))
}

pub fn branch_name(id: i64) -> String {
    format!("atalaya/mejora-{id}")
}

/// Rama principal del remoto (`main`, `master`…).
fn default_branch(repo: &Path) -> String {
    git(repo, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .ok()
        .and_then(|s| s.strip_prefix("origin/").map(String::from))
        .unwrap_or_else(|| "main".into())
}

/// Prepara una copia de trabajo limpia en una rama nueva, partiendo de lo último del remoto.
fn prepare(repo: &Path, improvement: &Improvement) -> Result<(PathBuf, String, String)> {
    let branch = branch_name(improvement.id);
    let dir = worktrees_dir()?.join(format!("{}-{}", improvement.slug, improvement.id));
    // Un intento anterior pudo dejar la copia o la rama: se parte de cero.
    if dir.exists() {
        let _ = git(repo, &["worktree", "remove", "--force", &dir.to_string_lossy()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
    let _ = git(repo, &["worktree", "prune"]);
    let _ = git(repo, &["branch", "-D", &branch]);

    let base = default_branch(repo);
    // Sin remoto (o sin red) se parte de la rama local.
    let start = match git(repo, &["fetch", "origin", &base]) {
        Ok(_) => format!("origin/{base}"),
        Err(_) => git(repo, &["rev-parse", "--abbrev-ref", "HEAD"])?,
    };
    std::fs::create_dir_all(dir.parent().context("sin carpeta")?)?;
    git(repo, &["worktree", "add", "-b", &branch, &dir.to_string_lossy(), &start])?;
    Ok((dir, branch, start))
}

/// El encargo de una mejora, igual para cualquier constructor.
fn task_text(improvement: &Improvement, project: &Project) -> String {
    let mut task = format!(
        "Mejora #{} del proyecto «{}».\n\nTítulo: {}\n\nDetalle: {}",
        improvement.id,
        project.name,
        improvement.title,
        if improvement.detail.is_empty() { "(sin detalle)" } else { &improvement.detail }
    );
    // Lo que el usuario ya respondió manda sobre el detalle: no se le vuelve a preguntar lo mismo.
    if let Some(feedback) = improvement.feedback.as_deref().filter(|f| !f.trim().is_empty()) {
        task.push_str(&format!(
            "\n\nConversación con el usuario sobre esta mejora (sus respuestas mandan; no vuelvas a preguntar lo que ya \
             respondió):\n\n{feedback}"
        ));
    }
    task
}

/// El texto de la línea que empieza con `label` («Pregunta:», «Sin cambios:»), aunque
/// venga en negrita o en minúscula.
fn marked(said: &str, label: &str) -> Option<String> {
    said.lines().map(str::trim).find_map(|l| {
        let plain = l.trim_start_matches(['*', '_', '-', ' ']);
        let head = plain.get(..label.len())?;
        head.eq_ignore_ascii_case(label)
            .then(|| plain[label.len()..].trim_start_matches(['*', '_']).trim().to_string())
    })
}

/// La pregunta con la que terminó un agente («Pregunta: …»), o su última línea.
fn question_in(said: &str) -> String {
    marked(said, "Pregunta:")
        .or_else(|| said.lines().map(str::trim).filter(|l| !l.is_empty()).last().map(String::from))
        .unwrap_or_default()
}

/// El motivo con el que un agente concluyó que no hacía falta cambiar nada («Sin cambios: …»).
fn no_changes_needed(said: &str) -> Option<String> {
    marked(said, "Sin cambios:").filter(|reason| !reason.is_empty())
}

/// Cuánto puede trabajar un agente de terminal en una mejora.
const AGENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45 * 60);

/// Hace el trabajo con el constructor elegido. Devuelve su informe.
fn build(
    registry: &Registry,
    builder: &Candidate,
    dir: &Path,
    task: &str,
    slug: &str,
    emit: &dyn Fn(&str, &str),
) -> Result<String> {
    match builder {
        // Se anota con la misma tarea que los demás constructores.
        Candidate::ClaudeCode => {
            crate::ai::tracked_claude(registry, "agente", Some(slug), |last| run_agent(registry, slug, dir, task, emit, last))
        }
        Candidate::Connection(c) if c.kind == "agente" => {
            emit("paso", &format!("{} trabaja dentro del aislamiento de macOS", c.name));
            let guidance = crate::skills::for_ai(registry, Some(slug), &c.id, crate::skills::Reader::Files)?;
            let prompt = format!("{RULES}{guidance}\n\n{task}");
            let run = crate::agents::run_in_worktree(registry, c, dir, &prompt, Some(slug), AGENT_TIMEOUT)?;
            let tail = clip_end(&run.output, 4000);
            if !run.ok {
                bail!(
                    "{}: {}",
                    if run.timed_out { "se acabó el tiempo" } else { "terminó con error" },
                    clip_end(&run.output, 600)
                );
            }
            Ok(tail)
        }
        Candidate::Connection(c) => bail!("«{}» no es un agente de terminal", c.name),
    }
}

/// Lo último de un texto largo.
fn clip_end(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    if chars.len() <= max {
        return chars.into_iter().collect();
    }
    format!("…{}", chars[chars.len() - max..].iter().collect::<String>())
}

/// El veredicto del revisor.
struct Review {
    by: String,
    /// `aprobado`, `cambios` o `sin_revision`.
    verdict: String,
    comments: String,
}

impl Review {
    fn heading(&self) -> String {
        match self.verdict.as_str() {
            "aprobado" => format!("Revisión de {}: aprobado", self.by),
            "cambios" => format!("Revisión de {}: pide cambios", self.by),
            _ => format!("Revisión: {}", self.by),
        }
    }
}

const REVIEW_SYSTEM: &str = "Eres el revisor de Atalaya. Otro modelo hizo un cambio en un repositorio para resolver \
una mejora y tú lo revisas antes de que llegue a una persona. Busca errores reales: lógica rota, casos no cubiertos, \
seguridad, secretos expuestos, cambios fuera del encargo o que rompan otra cosa. No pidas cambios de estilo ni \
preferencias. Responde SOLO con un objeto JSON, sin texto alrededor: \
{\"veredicto\": \"aprobado\" o \"cambios\", \"comentarios\": \"en español, texto simple, qué viste y, si pides \
cambios, qué corregir exactamente\"}.";

/// Pide al revisor (otro modelo) su veredicto sobre el trabajo de la rama.
fn review(
    registry: &Registry,
    builder: &Candidate,
    dir: &Path,
    start: &str,
    improvement: &Improvement,
    report: &str,
    emit: &dyn Fn(&str, &str),
) -> Option<Review> {
    if crate::orchestrator::config(registry, "revisor").ok()?.candidates.is_empty() {
        return None;
    }
    let diff = match git(dir, &["diff", &format!("{start}..HEAD")]) {
        Ok(d) => d,
        Err(e) => return Some(Review { by: "sin revisor".into(), verdict: "sin_revision".into(), comments: format!("{e:#}") }),
    };
    let diff: String = if diff.chars().count() > 60_000 {
        format!("{}\n… (diff recortado)", diff.chars().take(60_000).collect::<String>())
    } else {
        diff
    };
    let pack = crate::context::pack(registry, &improvement.slug).unwrap_or_default();
    let prompt = format!(
        "Mejora #{}: {}\n\nEncargo:\n{}\n\nInforme del constructor:\n{}\n\nContexto del proyecto (JSON):\n{}\n\nCambio (git diff):\n```diff\n{}\n```",
        improvement.id,
        improvement.title,
        if improvement.detail.is_empty() { "(sin detalle)" } else { &improvement.detail },
        report,
        pack,
        diff
    );
    let result = crate::orchestrator::attempt(registry, "revisor", std::slice::from_ref(builder), |reviewer| {
        emit("estado", &format!("{} revisa el cambio", reviewer.label()));
        crate::orchestrator::ask(registry, reviewer, REVIEW_SYSTEM, &prompt, "revision", Some(&improvement.slug))
    });
    Some(match result {
        Ok((text, reviewer)) => {
            let parsed = crate::orchestrator::extract_json(&text);
            let verdict = parsed
                .as_ref()
                .and_then(|v| v["veredicto"].as_str())
                .map(|v| if v.trim().eq_ignore_ascii_case("cambios") { "cambios" } else { "aprobado" })
                .unwrap_or("sin_revision");
            let comments = parsed
                .as_ref()
                .and_then(|v| v["comentarios"].as_str().map(String::from))
                .unwrap_or_else(|| text.trim().to_string());
            Review { by: reviewer.label(), verdict: verdict.into(), comments }
        }
        Err(e) => Review { by: "no se pudo revisar".into(), verdict: "sin_revision".into(), comments: format!("{e:#}") },
    })
}

/// Ejecuta a Claude Code en la copia de trabajo. Devuelve su resumen final.
fn run_agent(
    registry: &Registry,
    slug: &str,
    dir: &Path,
    task: &str,
    emit: &dyn Fn(&str, &str),
    last: &mut Option<Value>,
) -> Result<String> {
    let claude = find_claude().context("no se encontró Claude Code en esta máquina (comando `claude`)")?;
    // Las instrucciones del usuario van con las reglas; sus skills, como plugin de la sesión.
    let rules = format!("{RULES}{}", crate::skills::for_ai(registry, Some(slug), CLAUDE_CODE, crate::skills::Reader::Plugin)?);
    let plugin = crate::skills::plugin_dir(registry, Some(slug), CLAUDE_CODE)?;
    let mut command = Command::new(claude);
    if let Some(dir) = &plugin {
        command.arg("--plugin-dir").arg(dir);
    }
    let mut child = command
        .args(["-p", task, "--output-format", "stream-json", "--verbose"])
        .args(["--permission-mode", "acceptEdits"])
        .args(crate::ai::claude_cap_args(registry)?)
        .args(["--allowedTools", ALLOWED, "--disallowedTools", DENIED])
        .args(["--settings", SANDBOX])
        .env("PATH", developer_path())
        // Cerrojos de verdad, además de las reglas: aunque lo intentara, desde este
        // proceso no se puede subir al remoto ni desplegar con las sesiones guardadas.
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "remote.origin.pushurl")
        .env("GIT_CONFIG_VALUE_0", "bloqueado-por-atalaya://subir-requiere-tu-visto-bueno")
        .env("VERCEL_TOKEN", "bloqueado-por-atalaya")
        .env("FIREBASE_TOKEN", "bloqueado-por-atalaya")
        .args(["--append-system-prompt", &rules])
        // Sin servidores MCP: el agente solo trabaja sobre el código de la copia.
        .arg("--strict-mcp-config")
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout");
    let pid = child.id();
    RUNNING.lock().unwrap().push(pid);

    let mut summary = None;
    for line in BufReader::new(stdout).lines() {
        let Ok(event) = serde_json::from_str::<Value>(&line?) else { continue };
        match event["type"].as_str() {
            Some("assistant") => {
                for block in event["message"]["content"].as_array().into_iter().flatten() {
                    if block["type"] == "tool_use" {
                        emit("paso", &describe_tool(block));
                    } else if let Some(text) = block["text"].as_str() {
                        emit("nota", text);
                    }
                }
            }
            Some("result") => {
                let text = event["result"].as_str().unwrap_or_default().to_string();
                let failed = event["is_error"].as_bool().unwrap_or(false);
                let text = crate::ai::claude_cap_note(&event).unwrap_or(text);
                *last = Some(event);
                if failed {
                    bail!("Claude no pudo terminar: {text}");
                }
                summary = Some(text);
            }
            _ => {}
        }
    }
    let output = child.wait_with_output()?;
    RUNNING.lock().unwrap().retain(|p| *p != pid);
    summary.with_context(|| {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if detail.is_empty() { "Claude terminó sin responder".to_string() } else { detail }
    })
}

/// Una línea legible de lo que el agente está haciendo.
fn describe_tool(block: &Value) -> String {
    let input = &block["input"];
    let file = |key: &str| {
        input[key]
            .as_str()
            .and_then(|p| Path::new(p).file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    match block["name"].as_str().unwrap_or_default() {
        "Bash" => {
            let command: String = input["command"].as_str().unwrap_or_default().chars().take(90).collect();
            format!("Ejecuta: {command}")
        }
        "Edit" | "Write" => format!("Edita {}", file("file_path")),
        "Read" => format!("Lee {}", file("file_path")),
        "Grep" | "Glob" => "Busca en el código".to_string(),
        other => other.to_string(),
    }
}

/// Lleva una mejora aceptada hasta un pull request. `emit(tipo, texto)` recibe el
/// avance: `estado` (cambio de etapa), `paso` (acción del agente) y `nota` (texto suyo).
pub fn run(registry: &Registry, id: i64, emit: &dyn Fn(&str, &str)) -> Result<Improvement> {
    let improvement = registry.improvement(id)?;
    if !matches!(improvement.status.as_str(), "aceptada" | "en_curso") {
        bail!("la mejora {id} está '{}'; solo se trabajan las aceptadas", improvement.status);
    }
    let project = registry
        .get(&improvement.slug)?
        .with_context(|| format!("no existe el proyecto '{}'", improvement.slug))?;
    let repo = project
        .local_path
        .as_deref()
        .map(PathBuf::from)
        .filter(|p| p.join(".git").exists())
        .context("el proyecto no tiene una carpeta local con git en esta máquina; clónalo y regístralo para que el agente pueda trabajar")?;

    // Si algo falla, la mejora vuelve a "aceptada" con el motivo, lista para reintentar.
    let fail = |e: anyhow::Error| -> anyhow::Error {
        let _ = registry.update_improvement_run(id, "aceptada", None, None, Some(&format!("No se completó: {e:#}")));
        e
    };

    emit("estado", "Preparando una copia de trabajo en una rama nueva");
    let (dir, branch, start) = prepare(&repo, &improvement).map_err(fail)?;
    registry.update_improvement_run(id, "en_curso", Some(&branch), None, Some("El agente está trabajando…"))?;

    let task = task_text(&improvement, &project);
    // Quien termina sin cambios (se trabó, o preguntó algo) le pasa la mejora al siguiente de
    // la lista: solo si ninguno logra nada se le pide una respuesta al usuario.
    let mut idle: Vec<(String, String)> = Vec::new();
    // Quien concluye que no hace falta cambiar nada («Sin cambios: …») cierra la mejora: no se
    // le pasa al siguiente, que llegaría a lo mismo gastando otro intento.
    let mut settled = false;
    let attempt = crate::orchestrator::attempt(registry, "constructor", &[], |builder| {
        // Si un constructor anterior falló a medias, el siguiente parte de cero.
        let _ = git(&dir, &["reset", "--hard", &start]);
        let _ = git(&dir, &["clean", "-fd"]);
        emit("estado", &format!("{} está trabajando", builder.label()));
        let summary = build(registry, builder, &dir, &task, &improvement.slug, emit)?;
        let commits: usize = git(&dir, &["rev-list", "--count", &format!("{start}..HEAD")])?.parse().unwrap_or(0);
        if commits == 0 {
            if no_changes_needed(&summary).is_some() {
                settled = true;
                return Ok(summary);
            }
            emit("estado", &format!("{} terminó sin hacer cambios; sigue el siguiente", builder.label()));
            idle.push((builder.label(), summary));
            bail!("terminó sin hacer cambios");
        }
        Ok(summary)
    });
    let (mut summary, builder) = match attempt {
        Ok(done) => done,
        Err(e) => {
            let _ = git(&repo, &["worktree", "remove", "--force", &dir.to_string_lossy()]);
            let _ = git(&repo, &["branch", "-D", &branch]);
            // Lo que dijo el último que lo intentó (el más capaz suele ir al final de la lista).
            let result = match idle.last() {
                Some((who, said)) => {
                    let tried: Vec<&str> = idle.iter().map(|(w, _)| w.as_str()).collect();
                    format!("El agente no hizo cambios (lo intentaron: {}). Lo último que dijo {who}:\n\n{said}", tried.join(", "))
                }
                None => format!("No se completó: {e:#}"),
            };
            registry.update_improvement_run(id, "aceptada", None, None, Some(&result))?;
            registry.conn().execute("UPDATE improvements SET branch = NULL WHERE id = ?1", [id])?;
            emit("estado", "Nadie pudo terminarla: necesita tu respuesta");
            // Se avisa enseguida; el clic abre el Agente con la mejora lista para responder.
            let reason = idle.last().map(|(_, said)| question_in(said)).unwrap_or_else(|| format!("{e:#}"));
            crate::checks::notify(
                registry,
                &format!("agente:{id}"),
                &format!("{}: la mejora #{id} necesita tu respuesta", project.name),
                &reason.chars().take(180).collect::<String>(),
            );
            return registry.improvement(id);
        }
    };

    if settled {
        let _ = git(&repo, &["worktree", "remove", "--force", &dir.to_string_lossy()]);
        let _ = git(&repo, &["branch", "-D", &branch]);
        let reason = no_changes_needed(&summary).unwrap_or_default();
        let result = format!("Resuelta sin cambios en el código. {} concluyó: {reason}\n\n{summary}", builder.label());
        registry.update_improvement_run(id, "hecha", None, None, Some(&result))?;
        // La rama ya no existe: que la mejora no la siga nombrando.
        registry.conn().execute("UPDATE improvements SET branch = NULL WHERE id = ?1", [id])?;
        registry.record_event(&improvement.slug, &format!("mejora #{id}: resuelta sin cambios en el código"), "agente")?;
        let headline = format!("{}: la mejora #{id} no necesitó cambios", project.name);
        crate::checks::notify(registry, "mejoras", &headline, &reason.chars().take(180).collect::<String>());
        crate::checks::telegram(registry, &format!("{headline}. {reason}"));
        emit("estado", "Resuelta sin cambios en el código");
        return registry.improvement(id);
    }

    // Revisión cruzada: otro modelo mira el cambio. Si pide cambios, el constructor tiene un intento más.
    if let Some(mut verdict) = review(registry, &builder, &dir, &start, &improvement, &summary, emit) {
        if verdict.verdict == "cambios" {
            emit("estado", &format!("{} corrige lo que pidió el revisor", builder.label()));
            let fix = format!(
                "{task}\n\nYa trabajaste esta mejora y tus commits están en esta rama. Un revisor (otro modelo) pidió \
                 estos cambios; corrígelos y confirma con commits nuevos:\n{}",
                verdict.comments
            );
            match build(registry, &builder, &dir, &fix, &improvement.slug, emit) {
                Ok(second) => {
                    summary = format!("{summary}\n\nCorrección tras la revisión:\n{second}");
                    let first = format!("{}\n{}", verdict.heading(), verdict.comments);
                    if let Some(again) = review(registry, &builder, &dir, &start, &improvement, &summary, emit) {
                        verdict = again;
                    }
                    summary = format!("{summary}\n\nPrimera revisión — {first}");
                }
                Err(e) => summary = format!("{summary}\n\nNo se pudo aplicar la corrección pedida: {e:#}"),
            }
        }
        summary = format!("{summary}\n\n{}\n{}", verdict.heading(), verdict.comments);
        registry.record_event(&improvement.slug, &format!("mejora #{id}: {}", verdict.heading()), "agente")?;
    }

    // Antes de subir: que el cambio no lleve llaves, tokens ni archivos sensibles. Si los lleva
    // (o no se pudo revisar), la rama se queda en el Mac y la mejora explica qué encontró.
    emit("estado", "Revisando que el cambio no lleve secretos");
    let leaks = crate::leaks::scan_repo(&dir, &start, false);
    if !matches!(&leaks, Ok(found) if found.is_empty()) {
        let _ = git(&repo, &["worktree", "remove", "--force", &dir.to_string_lossy()]);
        let why = match leaks {
            Ok(found) => format!("la revisión de secretos encontró esto en el cambio:\n{}", crate::leaks::report(&found)),
            Err(e) => format!("no se pudo revisar si el cambio lleva secretos ({e:#})"),
        };
        let result = format!(
            "{summary}\n\nNo se subió: {why}\n\nEl trabajo quedó en la rama local {branch}, sin subir. Revísalo; si es un falso \
             positivo, marca la línea con `atalaya:permitir` y vuelve a mandarla, o súbela tú."
        );
        registry.update_improvement_run(id, "en_revision", Some(&branch), None, Some(&result))?;
        registry.record_event(&improvement.slug, &format!("mejora #{id}: no se subió, posible secreto en el cambio"), "agente")?;
        let headline = format!("{}: la mejora #{id} no se subió", project.name);
        crate::checks::notify(registry, "mejoras", &headline, "El cambio parece llevar una llave o un archivo sensible.");
        crate::checks::telegram(registry, &format!("{headline}: el cambio parece llevar una llave o un archivo sensible."));
        emit("estado", "No se subió: el cambio parece llevar secretos");
        return registry.improvement(id);
    }

    emit("estado", "Subiendo la rama y abriendo el pull request");
    let pushed = git(&dir, &["push", "-u", "origin", &branch]);
    let pr = match (&pushed, project.repo.as_deref()) {
        (Ok(_), Some(repo_name)) => open_pull_request(repo_name, &improvement, &branch, &summary),
        (Err(e), _) => Err(anyhow::anyhow!("no se pudo subir la rama: {e:#}")),
        (_, None) => Err(anyhow::anyhow!("el proyecto no tiene repo de GitHub registrado")),
    };
    // La copia de trabajo ya no hace falta; el trabajo queda en la rama.
    let _ = git(&repo, &["worktree", "remove", "--force", &dir.to_string_lossy()]);

    let (pr_url, result) = match pr {
        Ok(url) => (Some(url), summary),
        Err(e) => (
            None,
            format!("{summary}\n\nEl trabajo quedó en la rama local {branch}, pero no se abrió el pull request: {e:#}"),
        ),
    };
    registry.update_improvement_run(id, "en_revision", Some(&branch), pr_url.as_deref(), Some(&result))?;
    registry.record_event(&improvement.slug, &format!("mejora #{id}: trabajo del agente listo para revisión"), "agente")?;
    // El agente terminó: se avisa para que la revisión no dependa de mirar el tablero.
    let headline = format!("{}: mejora #{id} lista para tu revisión", project.name);
    crate::checks::notify(registry, "mejoras", &headline, &improvement.title);
    if crate::mail::configured(registry).unwrap_or(false) {
        let link = pr_url.as_deref().map(|u| format!("\n\nPull request: {u}")).unwrap_or_default();
        let body = format!("{}\n\n{result}{link}", improvement.title);
        if let Err(e) = crate::mail::send(registry, &format!("Atalaya: {headline}"), &body) {
            eprintln!("no se pudo enviar el correo: {e:#}");
        }
    }
    emit("estado", "Listo para tu revisión");
    registry.improvement(id)
}

fn open_pull_request(repo: &str, improvement: &Improvement, branch: &str, summary: &str) -> Result<String> {
    let (_, info) = github::api("GET", &format!("/repos/{repo}"), None)?;
    let base = info["default_branch"].as_str().unwrap_or("main");
    let body = json!({
        "title": improvement.title,
        "head": branch,
        "base": base,
        "body": format!(
            "Mejora #{} de Atalaya, trabajada por el agente.\n\n{}\n\n---\n{}",
            improvement.id, improvement.detail, summary
        ),
    });
    let (code, created) = github::api("POST", &format!("/repos/{repo}/pulls"), Some(&body))?;
    if code >= 400 {
        bail!("GitHub rechazó el pull request (error {code}): {}", created["message"].as_str().unwrap_or("sin detalle"));
    }
    created["html_url"].as_str().map(String::from).context("GitHub no devolvió la dirección del pull request")
}

/// Número del pull request a partir de su dirección.
fn pr_number(url: &str) -> Option<&str> {
    url.rsplit('/').next().filter(|n| n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty())
}

/// Fusiona el pull request de una mejora en revisión y la marca como hecha.
/// Fusionar lleva el cambio a la rama principal y, con ello, a producción.
pub fn merge(registry: &Registry, id: i64) -> Result<Improvement> {
    let improvement = registry.improvement(id)?;
    let project = registry.get(&improvement.slug)?.context("el proyecto ya no existe")?;
    let repo = project.repo.clone().context("el proyecto no tiene repo de GitHub")?;
    let url = improvement.pr_url.as_deref().context("la mejora no tiene pull request")?;
    let number = pr_number(url).context("no se reconoce el número del pull request")?;
    let (code, body) = github::api_put(&format!("/repos/{repo}/pulls/{number}/merge"), &json!({ "merge_method": "squash" }))?;
    if code >= 400 {
        bail!("GitHub no pudo fusionar (error {code}): {}", body["message"].as_str().unwrap_or("sin detalle"));
    }
    registry.update_improvement_run(id, "hecha", None, None, None)?;
    let local = crate::local_sync::track(registry, &project, improvement.branch.as_deref()).text;
    registry.record_event(&improvement.slug, &format!("mejora #{id}: pull request fusionado. {local}"), "app")?;
    registry.improvement(id)
}

/// Convierte una alerta en una mejora aceptada, para que el agente la resuelva.
/// La alerta queda reconocida mientras se trabaja: sigue vigilada, pero deja de
/// contar como abierta. Si ya hay una mejora viva para esa alerta, devuelve esa.
pub fn resolve_alert(registry: &Registry, slug: &str, kind: &str) -> Result<i64> {
    let check = registry
        .latest_checks(slug)?
        .into_iter()
        .find(|c| c.kind == kind)
        .with_context(|| format!("'{slug}' no tiene una revisión de tipo '{kind}'"))?;
    if check.status == crate::checks::OK {
        bail!("esa revisión ya está en orden; no hay nada que resolver");
    }
    let title = format!("Resolver alerta de {kind}");
    let alive = registry.improvements(Some(slug))?.into_iter().find(|i| {
        i.title == title && !matches!(i.status.as_str(), "hecha" | "descartada")
    });
    let id = match alive {
        Some(existing) => existing.id,
        None => {
            let detail = format!(
                "La vigilancia de Atalaya marcó esta revisión como «{}»: {}\n\nDetalle de la revisión:\n{}\n\n\
Resuelve la causa en el código del proyecto. Si la causa no está en el código (por ejemplo, algo que hay que \
hacer en un servicio en producción), no fuerces un cambio: explica con precisión qué hay que hacer y dónde.",
                check.status,
                check.summary,
                serde_json::to_string_pretty(&check.details).unwrap_or_default().chars().take(4000).collect::<String>(),
            );
            let id = registry.add_improvement(slug, &title, &detail, "alerta")?;
            registry.set_improvement_status(id, "aceptada")?;
            id
        }
    };
    registry.acknowledge(slug, kind)?;
    registry.record_event(slug, &format!("alerta de {kind} enviada a resolver como mejora #{id}"), "app")?;
    Ok(id)
}

/// Fusiona sola las mejoras en revisión cuando el usuario activó esa autonomía y las
/// comprobaciones del pull request pasaron todas. Sin comprobaciones no hay evidencia
/// de que el cambio funcione, así que esas quedan esperando revisión.
pub fn auto_merge(registry: &Registry) -> Result<usize> {
    if registry.setting(AUTO_MERGE)?.as_deref() != Some("si") {
        return Ok(0);
    }
    let mut merged = 0;
    for improvement in registry.improvements(None)? {
        if improvement.status != "en_revision" {
            continue;
        }
        let (Some(url), Some(project)) = (improvement.pr_url.as_deref(), registry.get(&improvement.slug)?) else {
            continue;
        };
        let (Some(number), Some(repo)) = (pr_number(url), project.repo.clone()) else { continue };
        let Ok((200, pr)) = github::api("GET", &format!("/repos/{repo}/pulls/{number}"), None) else { continue };
        if pr["state"] != "open" || pr["draft"] == true {
            continue;
        }
        let Some(sha) = pr["head"]["sha"].as_str() else { continue };
        if !checks_passed(&repo, sha) {
            continue;
        }
        if merge(registry, improvement.id).is_ok() {
            merged += 1;
            let title = format!("{}: mejora #{} fusionada sola", project.name, improvement.id);
            let body = format!("{} — las comprobaciones pasaron y va camino a producción.", improvement.title);
            crate::checks::notify(registry, "mejoras", &title, &body);
            crate::checks::telegram(registry, &format!("{title}\n{body}"));
        }
    }
    Ok(merged)
}

pub const AUTO_MERGE: &str = "auto_merge";

/// Todas las comprobaciones del commit terminaron bien, y hay al menos una.
fn checks_passed(repo: &str, sha: &str) -> bool {
    let Ok((200, status)) = github::api("GET", &format!("/repos/{repo}/commits/{sha}/status"), None) else {
        return false;
    };
    let Ok((200, runs)) = github::api("GET", &format!("/repos/{repo}/commits/{sha}/check-runs"), None) else {
        return false;
    };
    let statuses = status["total_count"].as_u64().unwrap_or(0);
    let runs = runs["check_runs"].as_array().cloned().unwrap_or_default();
    if statuses == 0 && runs.is_empty() {
        return false;
    }
    let statuses_ok = statuses == 0 || status["state"] == "success";
    let runs_ok = runs.iter().all(|r| {
        r["status"] == "completed"
            && matches!(r["conclusion"].as_str(), Some("success" | "neutral" | "skipped"))
    });
    statuses_ok && runs_ok
}

/// Pasa a "hecha" las mejoras en revisión cuyo pull request ya se fusionó en GitHub.
pub fn sync_merged(registry: &Registry) -> Result<usize> {
    let mut done = 0;
    for improvement in registry.improvements(None)? {
        if improvement.status != "en_revision" {
            continue;
        }
        let (Some(url), Some(project)) = (improvement.pr_url.as_deref(), registry.get(&improvement.slug)?) else {
            continue;
        };
        let (Some(number), Some(repo)) = (pr_number(url), project.repo.clone()) else { continue };
        let Ok((200, pr)) = github::api("GET", &format!("/repos/{repo}/pulls/{number}"), None) else { continue };
        if pr["merged"] == true {
            registry.update_improvement_run(improvement.id, "hecha", None, None, None)?;
            let local = crate::local_sync::track(registry, &project, improvement.branch.as_deref()).text;
            registry.record_event(
                &improvement.slug,
                &format!("mejora #{}: fusionada en GitHub. {local}", improvement.id),
                "vigía",
            )?;
            done += 1;
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {

    #[test]
    fn finds_the_question_an_agent_left() {
        assert_eq!(question_in("Revisé todo.\n**Pregunta:** ¿azul o verde?\nGracias"), "¿azul o verde?");
        assert_eq!(question_in("No pude abrir la terminal.\n¿Puedes habilitarla?"), "¿Puedes habilitarla?");
        assert_eq!(question_in(""), "");
    }

    #[test]
    fn knows_when_an_agent_concluded_nothing_needed_changing() {
        let said = "Revisé la carpeta.\n**Sin cambios:** la caché ya la ignora el .gitignore de main.\nFin";
        assert_eq!(no_changes_needed(said).as_deref(), Some("la caché ya la ignora el .gitignore de main."));
        assert_eq!(no_changes_needed("- sin cambios: ya estaba resuelto").as_deref(), Some("ya estaba resuelto"));
        assert_eq!(no_changes_needed("No hice cambios porque no pude abrir la terminal."), None);
        assert_eq!(no_changes_needed("Sin cambios:"), None, "sin motivo no cuenta");
        assert_eq!(no_changes_needed("Árbol: sin cambios"), None);
    }
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

    /// Enviar una alerta a resolver crea una sola mejora aceptada y reconoce la alerta.
    #[test]
    fn alert_becomes_one_accepted_improvement() {
        let dir = std::env::temp_dir().join(format!("atalaya-alerta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let registry = Registry::open(&dir.join("p.db")).unwrap();
        registry.upsert(&project(&dir)).unwrap();
        let check = |status: &str| crate::Check {
            slug: "demo".into(),
            kind: "dependencias".into(),
            status: status.into(),
            summary: "9 vulnerabilidades".into(),
            details: json!({ "vulnerabilidades": 9 }),
            checked_at: String::new(),
            acked: false,
        };
        registry.record_check(&check("falla")).unwrap();

        let id = resolve_alert(&registry, "demo", "dependencias").unwrap();
        let improvement = registry.improvement(id).unwrap();
        assert_eq!(improvement.status, "aceptada");
        assert!(improvement.detail.contains("9 vulnerabilidades"));
        assert!(registry.latest_checks("demo").unwrap()[0].acked, "queda reconocida");
        assert_eq!(resolve_alert(&registry, "demo", "dependencias").unwrap(), id, "no se duplica");
        assert!(resolve_alert(&registry, "demo", "sitio").is_err(), "sin esa revisión");

        registry.record_check(&check("ok")).unwrap();
        assert!(resolve_alert(&registry, "demo", "dependencias").is_err(), "ya está en orden");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_different_model_reviews_the_branch() {
        use crate::orchestrator::{RoleConfig, set_config};
        use crate::testutil;
        let r = testutil::registry("revision");
        testutil::project(&r, "tienda");
        let root = std::env::temp_dir().join(format!("atalaya-revision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        sh(&root, "git init -q -b main && git -c user.name=a -c user.email=a@b commit -q --allow-empty -m inicio");
        let start = git(&root, &["rev-parse", "HEAD"]).unwrap();
        sh(&root, "echo arreglo > a.txt && git add a.txt && git -c user.name=a -c user.email=a@b commit -q -m arreglo");
        let improvement = Improvement {
            id: 7,
            slug: "tienda".into(),
            title: "Arreglar".into(),
            detail: String::new(),
            status: "en_curso".into(),
            branch: None,
            pr_url: None,
            result: None,
            feedback: None,
            source: "app".into(),
            created_at: String::new(),
            updated_at: String::new(),
        };

        // Sin revisor asignado no se revisa.
        assert!(review(&r, &Candidate::ClaudeCode, &root, &start, &improvement, "hecho", &|_, _| {}).is_none());

        let url = testutil::chat_server(r#"{"veredicto": "cambios", "comentarios": "falta probar a.txt"}"#.into());
        let same = testutil::local_ai(&r, "Mismo", &url, "m1");
        let other = testutil::local_ai(&r, "Otro", &url, "m2");
        set_config(&r, "revisor", &RoleConfig { candidates: vec![same.id.clone(), other.id.clone()], strategy: "primero".into() })
            .unwrap();
        // El constructor fue «Mismo»: revisa «Otro».
        let builder = Candidate::Connection(same);
        let verdict = review(&r, &builder, &root, &start, &improvement, "hecho", &|_, _| {}).unwrap();
        assert_eq!(verdict.verdict, "cambios");
        assert_eq!(verdict.by, "Otro (m2)");
        assert_eq!(verdict.comments, "falta probar a.txt");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
