//! Vigilancia: revisiones periódicas de cada proyecto, con aviso cuando cambia el estado.

use crate::{Check, Project, Registry};
use anyhow::Result;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const OK: &str = "ok";
pub const WARN: &str = "aviso";
pub const FAIL: &str = "falla";

/// Tipos de revisión y cada cuántos segundos toca repetirlas.
pub const KINDS: [(&str, f64); 12] = [
    ("sitio", 300.0),
    ("certificado", 86400.0),
    ("migraciones", 3600.0),
    ("github", 600.0),
    ("firebase", 3600.0),
    ("despliegue", 300.0),
    ("supabase", 3600.0),
    ("git", 300.0),
    ("dependencias", 6.0 * 3600.0),
    ("base", 900.0),
    ("servidor", 600.0),
    ("repositorio", 600.0),
];

type Outcome = (&'static str, String, Value);

/// Ejecuta una revisión. `None` si no aplica a este proyecto.
fn run_one(registry: &Registry, project: &Project, kind: &str) -> Option<Outcome> {
    let path = project.local_path.as_deref().map(Path::new).filter(|p| p.is_dir());
    match kind {
        "sitio" => project.production_url.as_deref().map(site),
        "certificado" => project.production_url.as_deref().and_then(certificate),
        "migraciones" => path.and_then(|p| migrations(registry, project, p)),
        "despliegue" => deployment(registry, project),
        "supabase" => supabase(registry, project),
        "base" => own_databases(registry, project),
        "servidor" => servers(registry, project),
        "repositorio" => forges(registry, project),
        "github" => project.repo.as_deref().and_then(github),
        "firebase" => firebase(project),
        "git" => path.and_then(git),
        "dependencias" => path.filter(|p| p.join("package.json").is_file()).map(dependencies),
        _ => None,
    }
}

/// Vencimiento del certificado SSL del sitio. Usa `openssl`, que viene con macOS y Linux.
fn certificate(url: &str) -> Option<Outcome> {
    let host = url.strip_prefix("https://")?.split(['/', ':']).next()?.to_string();
    let shell = |script: &str| {
        Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("SITIO", &host)
            .output()
            .ok()
    };
    // El certificado se pide una vez y se examina con varios plazos.
    let fetch = "echo | openssl s_client -servername \"$SITIO\" -connect \"$SITIO:443\" 2>/dev/null";
    let out = shell(&format!("{fetch} | openssl x509 -noout -enddate"))?;
    let expires = String::from_utf8_lossy(&out.stdout)
        .trim()
        .strip_prefix("notAfter=")
        .map(String::from);
    let Some(expires) = expires else {
        return Some((WARN, "No se pudo leer el certificado".into(), json!({ "sitio": host })));
    };
    let valid_for = |days: u64| {
        shell(&format!("{fetch} | openssl x509 -noout -checkend {}", days * 86400))
            .is_some_and(|o| o.status.success())
    };
    let details = json!({ "sitio": host, "vence": expires });
    Some(if !valid_for(3) {
        (FAIL, format!("El certificado vence en menos de 3 días ({expires})"), details)
    } else if !valid_for(21) {
        (WARN, format!("El certificado vence en menos de 3 semanas ({expires})"), details)
    } else {
        (OK, format!("Certificado vigente hasta {expires}"), details)
    })
}

/// Migraciones que están en el repo pero no aplicadas en la base de producción.
/// Entiende Drizzle (`drizzle/meta/_journal.json`) y Supabase (`supabase/migrations`).
/// No aplica si el proyecto no usa ninguno de los dos o no hay llave de Supabase.
fn migrations(registry: &Registry, project: &Project, path: &Path) -> Option<Outcome> {
    let reference = supabase_ref(registry, project)?;
    let token = crate::connections::token_for(registry, Some(&project.slug), "supabase")?;
    let unreachable = |e: anyhow::Error| {
        Some((WARN, "No se pudo comparar las migraciones".to_string(), json!({ "error": format!("{e:#}") })))
    };

    let journal = std::fs::read_to_string(path.join("drizzle/meta/_journal.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let (pending, total): (Vec<String>, usize) = if let Some(journal) = journal {
        let entries = journal["entries"].as_array().cloned().unwrap_or_default();
        // Drizzle anota cada migración aplicada con la marca de tiempo de su entrada.
        let rows = match crate::supabase::query_with(
            &token,
            &reference,
            "select created_at from drizzle.__drizzle_migrations",
        ) {
            Ok(rows) => rows,
            Err(e) => return unreachable(e),
        };
        let applied: Vec<i64> = rows
            .iter()
            .filter_map(|r| r["created_at"].as_i64().or_else(|| r["created_at"].as_str()?.parse().ok()))
            .collect();
        let pending = entries
            .iter()
            .filter(|e| e["when"].as_i64().is_some_and(|when| !applied.contains(&when)))
            .filter_map(|e| e["tag"].as_str().map(String::from))
            .collect();
        (pending, entries.len())
    } else if path.join("supabase/migrations").is_dir() {
        let mut local: Vec<String> = std::fs::read_dir(path.join("supabase/migrations"))
            .ok()?
            .filter_map(|f| f.ok()?.file_name().into_string().ok())
            .filter(|name| name.ends_with(".sql"))
            .collect();
        local.sort();
        let rows = match crate::supabase::query_with(
            &token,
            &reference,
            "select version from supabase_migrations.schema_migrations",
        ) {
            Ok(rows) => rows,
            Err(e) => return unreachable(e),
        };
        let applied: Vec<String> = rows.iter().filter_map(|r| r["version"].as_str().map(String::from)).collect();
        let total = local.len();
        // El nombre del archivo empieza por su versión: "20261101120000_nombre.sql".
        let pending = local
            .into_iter()
            .filter(|name| !applied.iter().any(|v| name.starts_with(v.as_str())))
            .collect();
        (pending, total)
    } else {
        return None;
    };

    let details = json!({ "en_el_repo": total, "sin_aplicar": pending });
    Some(if pending.is_empty() {
        (OK, format!("Las {total} migraciones del repo están aplicadas en producción"), details)
    } else {
        let names = pending.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        let more = if pending.len() > 3 { "…" } else { "" };
        (
            WARN,
            format!("{} migraciones sin aplicar en producción: {names}{more}", pending.len()),
            details,
        )
    })
}

/// La referencia de Supabase de un proyecto: la del vínculo o la detectada en el repo.
pub(crate) fn supabase_ref(registry: &Registry, project: &Project) -> Option<String> {
    crate::connections::linked(registry, &project.slug, "base", Some("supabase"))
        .unwrap_or_default()
        .iter()
        .find_map(|(l, _)| l.config_str("ref"))
        .or_else(|| crate::inspect::project_services(project).supabase_project_ref)
}

/// Bases propias vinculadas (Postgres, MySQL/MariaDB, SQLite): conexión, tamaño y latencia.
fn own_databases(registry: &Registry, project: &Project) -> Option<Outcome> {
    let parts = crate::connections::linked(registry, &project.slug, "base", None)
        .unwrap_or_default()
        .iter()
        .filter(|(_, c)| matches!(c.provider.as_str(), "postgres" | "mysql" | "sqlite"))
        .map(|(l, c)| (c.name.clone(), crate::providers::database::check(c, l)))
        .collect();
    crate::providers::combine(parts)
}

/// Servidores propios vinculados como hosting: URL de salud, carga y disco.
fn servers(registry: &Registry, project: &Project) -> Option<Outcome> {
    let parts = crate::connections::linked(registry, &project.slug, "hosting", Some("servidor"))
        .unwrap_or_default()
        .iter()
        .filter_map(|(_, c)| Some((c.name.clone(), crate::providers::hosting::server_check(c)?)))
        .collect();
    crate::providers::combine(parts)
}

/// Repos en GitLab o Gitea/Forgejo vinculados: último pipeline y lo que está abierto.
fn forges(registry: &Registry, project: &Project) -> Option<Outcome> {
    let parts = crate::connections::linked(registry, &project.slug, "repo", None)
        .unwrap_or_default()
        .iter()
        .filter_map(|(l, c)| Some((c.name.clone(), crate::providers::forge::check(c, l)?)))
        .collect();
    crate::providers::combine(parts)
}

fn site(url: &str) -> Outcome {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .user_agent("Atalaya")
        .build()
        .into();
    let start = Instant::now();
    match agent.get(url).call() {
        Ok(res) => {
            let code = res.status().as_u16();
            let ms = start.elapsed().as_millis() as u64;
            let details = json!({ "url": url, "codigo": code, "ms": ms });
            if code >= 400 {
                (FAIL, format!("Responde con error {code}"), details)
            } else if ms > 5000 {
                (WARN, format!("En línea pero lento ({ms} ms)"), details)
            } else {
                (OK, format!("En línea ({ms} ms)"), details)
            }
        }
        Err(e) => (
            FAIL,
            "No responde".into(),
            json!({ "url": url, "error": e.to_string() }),
        ),
    }
}

/// Último despliegue de producción: en Netlify si el proyecto tiene un sitio vinculado,
/// si no en Vercel. No aplica si no se conoce el proyecto ni hay llave.
fn deployment(registry: &Registry, project: &Project) -> Option<Outcome> {
    let netlify: Vec<(String, Outcome)> = crate::connections::linked(registry, &project.slug, "hosting", Some("netlify"))
        .unwrap_or_default()
        .iter()
        .filter_map(|(link, c)| Some((c.name.clone(), crate::providers::hosting::netlify_check(c, link)?)))
        .collect();
    if !netlify.is_empty() {
        return crate::providers::combine(netlify);
    }
    let services = crate::inspect::project_services(project);
    let vercel_link = crate::connections::linked(registry, &project.slug, "hosting", Some("vercel"))
        .unwrap_or_default()
        .into_iter()
        .next();
    let project_id = vercel_link
        .as_ref()
        .and_then(|(l, _)| l.config_str("project_id"))
        .or(services.vercel_project_id)?;
    let token = crate::connections::token_for(registry, Some(&project.slug), "vercel")?;
    // El equipo: el del vínculo, el detectado en el repo o el de la conexión.
    let team = vercel_link
        .as_ref()
        .and_then(|(l, _)| l.config_str("team_id"))
        .or(services.vercel_team_id)
        .or_else(|| vercel_link.as_ref().and_then(|(_, c)| c.config_str("team_id")))
        .or_else(|| {
            crate::connections::default_for(registry, "vercel").ok().flatten().and_then(|c| c.config_str("team_id"))
        });

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut request = agent
        .get("https://api.vercel.com/v7/deployments")
        .query("projectId", &project_id)
        .query("target", "production")
        .query("limit", "1")
        .header("Authorization", &format!("Bearer {token}"));
    // Los proyectos de una cuenta personal no llevan equipo.
    if let Some(team) = team.filter(|t| t.starts_with("team_")) {
        request = request.query("teamId", &team);
    }

    let mut response = match request.call() {
        Ok(r) => r,
        Err(e) => {
            return Some((
                WARN,
                "No se pudo consultar Vercel".into(),
                json!({ "error": e.to_string() }),
            ));
        }
    };
    let code = response.status().as_u16();
    if code == 401 || code == 403 {
        return Some((
            WARN,
            "Vercel rechazó la llave guardada".into(),
            json!({ "codigo": code }),
        ));
    }
    let body: Value = response
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    if code >= 400 {
        return Some((
            WARN,
            format!("Vercel respondió con error {code}"),
            json!({ "codigo": code, "respuesta": body["error"] }),
        ));
    }
    let Some(last) = body["deployments"].get(0) else {
        return Some((WARN, "Sin despliegues de producción".into(), Value::Null));
    };
    let state = last["state"]
        .as_str()
        .or(last["readyState"].as_str())
        .unwrap_or("DESCONOCIDO");
    let commit = last["meta"]["githubCommitMessage"]
        .as_str()
        .and_then(|m| m.lines().next())
        .unwrap_or_default();
    let details = json!({
        "estado": state,
        "url": last["url"],
        "creado_ms": last["created"],
        "commit": commit,
    });
    let suffix = if commit.is_empty() {
        String::new()
    } else {
        format!(": {commit}")
    };
    Some(match state {
        "READY" => (OK, format!("Último despliegue correcto{suffix}"), details),
        "BUILDING" | "QUEUED" | "INITIALIZING" => (OK, format!("Desplegando{suffix}"), details),
        "ERROR" => (FAIL, format!("El último despliegue falló{suffix}"), details),
        "CANCELED" => (WARN, format!("El último despliegue se canceló{suffix}"), details),
        other => (WARN, format!("Despliegue en estado {other}"), details),
    })
}

/// Estado del proyecto de Firebase y de sus reglas de seguridad. No aplica si el
/// proyecto no usa Firebase o no hay cuenta de servicio (Conexiones → Bases de datos → Firebase).
fn firebase(project: &Project) -> Option<Outcome> {
    let id = crate::inspect::project_services(project).firebase_project?;
    if !crate::firebase::connected() {
        return None;
    }
    let folder = project.local_path.as_deref().map(Path::new).filter(|p| p.is_dir());
    let details = match crate::firebase::status(&id, folder) {
        Ok(details) => details,
        Err(e) => {
            return Some((
                WARN,
                "No se pudo consultar Firebase".into(),
                json!({ "error": format!("{e:#}") }),
            ));
        }
    };
    let state = details["estado"].as_str().unwrap_or_default();
    if state != "ACTIVE" {
        return Some((FAIL, format!("El proyecto de Firebase está en estado {state}"), details));
    }
    let rules = details["reglas"].as_array().cloned().unwrap_or_default();
    let names = |filter: &dyn Fn(&Value) -> bool| {
        rules
            .iter()
            .filter(|r| filter(r))
            .filter_map(|r| r["servicio"].as_str())
            .collect::<Vec<_>>()
            .join(" y ")
    };
    let open = names(&|r| r["escrituras_publicas"].as_u64().unwrap_or(0) > 0);
    let drift = names(&|r| r["igual_al_repo"] == false);
    if !open.is_empty() {
        return Some((
            FAIL,
            format!("Proyecto activo; las reglas de {open} permiten escribir a cualquiera"),
            details,
        ));
    }
    if !drift.is_empty() {
        return Some((
            WARN,
            format!("Proyecto activo; las reglas de {drift} desplegadas difieren de las del repo"),
            details,
        ));
    }
    let public_reads: u64 = rules.iter().filter_map(|r| r["lecturas_publicas"].as_u64()).sum();
    let mut summary = if rules.is_empty() {
        "Proyecto activo; sin reglas desplegadas".to_string()
    } else {
        format!("Proyecto activo; reglas de {} iguales al repo y sin escritura pública", names(&|_| true))
    };
    if public_reads > 0 {
        summary.push_str(&format!(" · {public_reads} lecturas públicas"));
    }
    Some((if rules.is_empty() { WARN } else { OK }, summary, details))
}

/// Estado del repo en GitHub. Funciona aunque el proyecto no tenga carpeta en esta
/// máquina. No aplica si no hay acceso a GitHub configurado.
fn github(repo: &str) -> Option<Outcome> {
    crate::github::token()?;
    let details = match crate::github::status(repo) {
        Ok(details) => details,
        Err(e) => {
            return Some((
                WARN,
                "No se pudo consultar GitHub".into(),
                json!({ "error": format!("{e:#}") }),
            ));
        }
    };
    let last = details["commits"][0]["mensaje"].as_str().unwrap_or_default().to_string();
    let open = details["abiertos"].as_u64().unwrap_or(0);
    let mut summary = match details["comprobaciones"].as_str().unwrap_or_default() {
        "failure" | "error" => "Las comprobaciones del último commit fallaron".to_string(),
        "pending" => "Comprobaciones del último commit en curso".to_string(),
        "success" => "Comprobaciones del último commit correctas".to_string(),
        _ => "Sin comprobaciones automáticas".to_string(),
    };
    if !last.is_empty() {
        summary.push_str(&format!(": {last}"));
    }
    if open > 0 {
        summary.push_str(&format!(" · {open} issues o PR abiertos"));
    }
    let failed = matches!(details["comprobaciones"].as_str(), Some("failure" | "error"));
    Some((if failed { FAIL } else { OK }, summary, details))
}

/// GET autenticado a una API. Devuelve el código HTTP y el cuerpo como JSON.
fn api_get(url: &str, token: &str) -> Result<(u16, Value), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .get(url)
        .header("Authorization", &format!("Bearer {token}"))
        .call()
        .map_err(|e| e.to_string())?;
    let body = response
        .body_mut()
        .read_to_string()
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    Ok((response.status().as_u16(), body))
}

/// Proyectos de Supabase visibles con la llave guardada: (nombre, referencia, organización, estado).
pub fn supabase_projects() -> Result<Vec<(String, String, String, String)>> {
    let token = crate::connections::token_for_default("supabase")
        .ok_or_else(|| anyhow::anyhow!("falta la llave de Supabase: agrégala en Conexiones"))?;
    let (code, body) =
        api_get("https://api.supabase.com/v1/projects", &token).map_err(anyhow::Error::msg)?;
    if code != 200 {
        anyhow::bail!("Supabase respondió con error {code}");
    }
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    Ok(body
        .as_array()
        .map(|list| {
            list.iter()
                .map(|p| {
                    let reference = if p["ref"].is_string() { &p["ref"] } else { &p["id"] };
                    (text(&p["name"]), text(reference), text(&p["organization_id"]), text(&p["status"]))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Cuenta los avisos de un asesor de Supabase por nivel: (errores, advertencias).
fn count_lints(body: &Value) -> (usize, usize, Vec<Value>) {
    let empty = Vec::new();
    let lints = body["lints"].as_array().or(body.as_array()).unwrap_or(&empty);
    let level = |l: &Value, want: &str| l["level"].as_str() == Some(want);
    let titles = lints
        .iter()
        .filter(|l| !level(l, "INFO"))
        .map(|l| json!({ "nivel": l["level"], "titulo": l["title"], "detalle": l["detail"] }))
        .collect();
    (
        lints.iter().filter(|l| level(l, "ERROR")).count(),
        lints.iter().filter(|l| level(l, "WARN")).count(),
        titles,
    )
}

/// Estado del proyecto de Supabase y avisos de sus asesores de seguridad y rendimiento.
/// No aplica si no se detecta el proyecto o no hay llave (Conexiones → Bases de datos → Supabase).
fn supabase(registry: &Registry, project: &Project) -> Option<Outcome> {
    let reference = supabase_ref(registry, project)?;
    let token = crate::connections::token_for(registry, Some(&project.slug), "supabase")?;
    let base = format!("https://api.supabase.com/v1/projects/{reference}");
    let unreachable = |e: String| {
        Some((WARN, "No se pudo consultar Supabase".to_string(), json!({ "error": e })))
    };

    let (code, project) = match api_get(&base, &token) {
        Ok(r) => r,
        Err(e) => return unreachable(e),
    };
    if code == 401 || code == 403 {
        return Some((
            WARN,
            "Supabase rechazó la llave guardada".into(),
            json!({ "codigo": code }),
        ));
    }
    if code >= 400 {
        return Some((
            WARN,
            format!("Supabase respondió con error {code}"),
            json!({ "codigo": code, "respuesta": project["message"] }),
        ));
    }
    let state = project["status"].as_str().unwrap_or("DESCONOCIDO").to_string();
    if state != "ACTIVE_HEALTHY" {
        let summary = match state.as_str() {
            "INACTIVE" | "PAUSING" => "La base de datos está pausada".to_string(),
            other => format!("La base de datos está en estado {other}"),
        };
        return Some((FAIL, summary, json!({ "estado": state })));
    }

    let advisor = |kind: &str| api_get(&format!("{base}/advisors/{kind}"), &token);
    let (security, performance) = match (advisor("security"), advisor("performance")) {
        (Ok((200, s)), Ok((200, p))) => (count_lints(&s), count_lints(&p)),
        (Err(e), _) | (_, Err(e)) => return unreachable(e),
        (Ok((sc, _)), Ok((pc, _))) => {
            return Some((
                WARN,
                "Base activa, pero no se pudieron leer los asesores".into(),
                json!({ "estado": state, "codigo_seguridad": sc, "codigo_rendimiento": pc }),
            ));
        }
    };

    let mut parts = Vec::new();
    if security.0 > 0 {
        parts.push(format!("{} errores de seguridad", security.0));
    }
    if security.1 > 0 {
        parts.push(format!("{} advertencias de seguridad", security.1));
    }
    if performance.0 + performance.1 > 0 {
        parts.push(format!("{} avisos de rendimiento", performance.0 + performance.1));
    }
    // El plan gratuito de Supabase incluye 500 MB de base de datos.
    let megabytes = crate::supabase::database_bytes_with(&token, &reference).ok().map(|b| b as f64 / 1_048_576.0);
    if let Some(mb) = megabytes.filter(|mb| *mb >= 400.0) {
        parts.push(format!("la base pesa {mb:.0} MB de los 500 del plan gratuito"));
    }
    let details = json!({
        "estado": state,
        "base_mb": megabytes.map(|mb| (mb * 10.0).round() / 10.0),
        "seguridad": security.2,
        "rendimiento": performance.2,
    });
    Some(if parts.is_empty() {
        let size = megabytes.map(|mb| format!(" · {mb:.0} MB")).unwrap_or_default();
        (OK, format!("Base activa y sin avisos{size}"), details)
    } else {
        let status = if security.0 > 0 { FAIL } else { WARN };
        (status, format!("Base activa; {}", parts.join(", ")), details)
    })
}

fn git(path: &Path) -> Option<Outcome> {
    let info = crate::inspect::git_info(path, 0)?;
    let unpushed = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-list", "--count", "@{u}..HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .unwrap_or(0);
    // Con nombres y tipo («supabase/.temp/ (nuevo)»), para saber de un vistazo si es trabajo o ruido.
    let files = crate::local_sync::pending_files(path);
    let mut parts = Vec::new();
    match files.len() {
        0 => {}
        1 => parts.push(format!("1 archivo sin confirmar: {}", files[0])),
        n => parts.push(format!("{n} archivos sin confirmar: {}", crate::local_sync::short_list(&files))),
    }
    match unpushed {
        0 => {}
        1 => parts.push("1 commit sin subir".to_string()),
        n => parts.push(format!("{n} commits sin subir")),
    }
    let details = json!({
        "rama": info.branch,
        "sin_confirmar": files.len(),
        "archivos": files.iter().take(20).collect::<Vec<_>>(),
        "sin_subir": unpushed,
    });
    Some(if parts.is_empty() {
        (OK, "Todo confirmado y subido".into(), details)
    } else {
        (WARN, parts.join(", "), details)
    })
}

/// Busca npm en el PATH y en las ubicaciones habituales: una app abierta
/// desde Finder o un servicio de sistema no heredan el PATH de la terminal.
fn find_npm() -> Option<PathBuf> {
    let from_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default();
    from_path
        .into_iter()
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from))
        .map(|dir| dir.join("npm"))
        .find(|p| p.is_file())
}

fn npm_json(npm: &Path, project: &Path, args: &[&str]) -> Option<Value> {
    let mut path = npm.parent()?.as_os_str().to_owned();
    if let Some(rest) = std::env::var_os("PATH") {
        path.push(":");
        path.push(rest);
    }
    // npm sale con código distinto de cero cuando encuentra algo; lo que importa es el JSON.
    let out = Command::new(npm)
        .current_dir(project)
        .env("PATH", path)
        .args(args)
        .arg("--json")
        .output()
        .ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

fn major(version: &str) -> Option<u64> {
    version.split('.').next()?.parse().ok()
}

fn dependencies(path: &Path) -> Outcome {
    let Some(npm) = find_npm() else {
        return (WARN, "No se encontró npm en esta máquina".into(), Value::Null);
    };

    // Desactualizadas: solo cuentan las que tienen una versión más nueva que la instalada.
    let mut outdated = Vec::new();
    let mut majors = 0;
    if let Some(Value::Object(map)) = npm_json(&npm, path, &["outdated"]) {
        for (name, info) in map {
            let (Some(current), Some(latest)) = (info["current"].as_str(), info["latest"].as_str())
            else {
                continue;
            };
            let (Some(cur), Some(lat)) = (major(current), major(latest)) else {
                continue;
            };
            if lat < cur || current == latest {
                continue;
            }
            if lat > cur {
                majors += 1;
            }
            outdated.push(json!({ "paquete": name, "instalada": current, "ultima": latest }));
        }
    }

    let audit = npm_json(&npm, path, &["audit"]);
    let vulns = audit
        .as_ref()
        .map(|a| &a["metadata"]["vulnerabilities"])
        .filter(|v| v.is_object());
    let count = |level: &str| vulns.and_then(|v| v[level].as_u64()).unwrap_or(0);
    let total = count("total");
    let serious = count("high") + count("critical");

    let mut parts = Vec::new();
    if total > 0 {
        parts.push(if serious > 0 {
            format!("{total} vulnerabilidades ({serious} graves)")
        } else {
            format!("{total} vulnerabilidades")
        });
    }
    if !outdated.is_empty() {
        parts.push(if majors > 0 {
            format!("{} desactualizadas ({majors} con salto mayor)", outdated.len())
        } else {
            format!("{} desactualizadas", outdated.len())
        });
    }
    if vulns.is_none() {
        parts.push("no se pudo revisar vulnerabilidades".into());
    }

    let details = json!({ "vulnerabilidades": vulns, "desactualizadas": outdated });
    let status = if serious > 0 {
        FAIL
    } else if total > 0 || majors > 0 || vulns.is_none() {
        WARN
    } else {
        OK
    };
    let summary = if parts.is_empty() {
        "Al día y sin vulnerabilidades".into()
    } else {
        parts.join(", ")
    };
    (status, summary, details)
}

/// Ejecuta las revisiones que tocan (o todas, con `force`) y las guarda.
/// Devuelve las ejecutadas junto con el estado anterior de cada una, si lo había.
pub fn run_due(
    registry: &Registry,
    only: Option<&str>,
    force: bool,
) -> Result<Vec<(Check, Option<String>)>> {
    let mut done = Vec::new();
    for project in registry.list()? {
        if only.is_some_and(|slug| slug != project.slug) {
            continue;
        }
        for (kind, every) in KINDS {
            let last = registry.last_check(&project.slug, kind)?;
            if !force && last.as_ref().is_some_and(|(_, age)| *age < every) {
                continue;
            }
            let Some((status, summary, details)) = run_one(registry, &project, kind) else {
                continue;
            };
            let check = Check {
                slug: project.slug.clone(),
                kind: kind.into(),
                status: status.into(),
                summary,
                details,
                checked_at: String::new(),
                acked: false,
            };
            registry.record_check(&check)?;
            done.push((check, last.map(|(status, _)| status)));
        }
    }
    Ok(done)
}

/// Avisa solo cuando el estado cambia: una falla nueva, un aviso nuevo o una recuperación.
/// La primera revisión de un proyecto solo avisa si encuentra una falla.
/// Cada cambio va como notificación del sistema; todos juntos, en un solo correo.
pub fn notify_changes(registry: &Registry, results: &[(Check, Option<String>)]) -> Result<()> {
    let mut lines = Vec::new();
    for (check, previous) in results {
        let changed = match previous.as_deref() {
            Some(prev) => prev != check.status,
            None => check.status == FAIL,
        };
        if !changed {
            continue;
        }
        let name = registry
            .get(&check.slug)?
            .map(|p| p.name)
            .unwrap_or_else(|| check.slug.clone());
        let title = match check.status.as_str() {
            OK => format!("{name}: {} recuperado", check.kind),
            WARN => format!("{name}: aviso en {}", check.kind),
            _ => format!("{name}: falla en {}", check.kind),
        };
        notify(registry, "alertas", &title, &check.summary);
        // Al teléfono solo lo grave: una falla nueva o su recuperación.
        if check.status == FAIL || previous.as_deref() == Some(FAIL) {
            telegram(registry, &format!("{title}\n{}", check.summary));
        }
        lines.push(format!("{title}\n  {}", check.summary));
    }
    if !lines.is_empty() && crate::mail::configured(registry)? {
        let subject = match lines.as_slice() {
            [one] => format!("Atalaya: {}", one.lines().next().unwrap_or_default()),
            many => format!("Atalaya: {} cambios de estado", many.len()),
        };
        // Un correo que no sale no debe impedir que la ronda termine.
        if let Err(e) = crate::mail::send(registry, &subject, &lines.join("\n\n")) {
            eprintln!("no se pudo enviar el correo: {e:#}");
        }
    }
    Ok(())
}

/// Mensaje al teléfono por Telegram, si hay bot y chat configurados
/// (Conexiones → Avisos → Telegram). Un fallo al enviar no interrumpe nada.
pub fn telegram(registry: &Registry, text: &str) -> bool {
    // El chat primero: sin Telegram configurado no se consulta el Llavero.
    let Ok(Some(chat)) = registry.setting("telegram_chat") else { return false };
    let Some(token) = crate::secrets::get("telegram") else { return false };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .http_status_as_error(false)
        .build()
        .into();
    let body = json!({ "chat_id": chat, "text": format!("Atalaya\n{text}") });
    agent
        .post(&format!("https://api.telegram.org/bot{token}/sendMessage"))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .is_ok_and(|r| r.status().as_u16() == 200)
}

/// Dónde queda anotada la vista que abre la última notificación.
pub const NOTICE: &str = "notice_view";

/// La app a cuyo nombre salen las notificaciones: su ícono, y la que se abre al hacer clic.
/// La app la fija con su identificador al arrancar; la terminal y el servicio usan la instalada.
pub const APP_ID_ENV: &str = "ATALAYA_APP_ID";
#[cfg(all(target_os = "macos", not(test)))]
const APP_ID: &str = "cl.thrazos.atalaya";

/// Notificación del sistema a nombre de Atalaya. Al hacer clic se abre Atalaya en `view`
/// (decisiones, mejoras, alertas, radar o consumo). Fuera de macOS no hace nada.
pub fn notify(registry: &Registry, view: &str, title: &str, body: &str) {
    let notice = json!({ "view": view, "at": registry.utc_now().unwrap_or_default() });
    if let Err(e) = registry.set_setting(NOTICE, &notice.to_string()) {
        eprintln!("no se pudo anotar la vista de la notificación: {e:#}");
    }
    post(title, body);
}

/// La vista que pidió abrir una notificación de la última hora. Se entrega una sola vez.
pub fn take_notice(registry: &Registry) -> Result<Option<String>> {
    let Some(raw) = registry.setting(NOTICE)?.filter(|r| !r.is_empty()) else { return Ok(None) };
    registry.set_setting(NOTICE, "")?;
    let notice: Value = serde_json::from_str(&raw).unwrap_or_default();
    let (Some(view), Some(at)) = (notice["view"].as_str(), notice["at"].as_str()) else { return Ok(None) };
    Ok((registry.seconds_since(at)? <= 3600.0).then(|| view.to_string()))
}

#[cfg(all(target_os = "macos", not(test)))]
fn post(title: &str, body: &str) {
    use mac_notification_sys::{Notification, set_application};
    use std::sync::Once;
    static APP: Once = Once::new();
    APP.call_once(|| {
        let app = std::env::var(APP_ID_ENV).ok().filter(|a| !a.is_empty()).unwrap_or_else(|| APP_ID.into());
        if let Err(e) = set_application(&app) {
            eprintln!("las notificaciones no salen a nombre de {app}: {e}");
        }
    });
    let sent = Notification::new().title("Atalaya").subtitle(title).message(body).asynchronous(true).send();
    if let Err(e) = sent {
        eprintln!("no se pudo mostrar la notificación: {e}");
    }
}

/// En las pruebas, y fuera de macOS, no se muestra nada.
#[cfg(any(not(target_os = "macos"), test))]
fn post(title: &str, body: &str) {
    let _ = (title, body);
}

/// Reporte en Markdown con el último estado de todos los proyectos.
pub fn report(registry: &Registry) -> Result<String> {
    let mut out = String::from("# Reporte de Atalaya\n");
    // Consumo de la API de Claude, si está conectada.
    let claude = crate::claude::status(registry)?;
    if let Some(spent) = claude["summary"]["month_usd"].as_f64() {
        out.push_str(&format!("\n## Claude Platform\n\n- Gasto del mes: US$ {spent:.2}"));
        if let Some(budget) = claude["budget_usd"].as_f64().filter(|b| *b > 0.0) {
            out.push_str(&format!(" de US$ {budget:.2} ({:.0} %)", spent / budget * 100.0));
        }
        out.push_str(&format!(
            "\n- Solicitudes: {}\n",
            claude["summary"]["requests"].as_u64().unwrap_or(0)
        ));
    }
    for project in registry.list()? {
        out.push_str(&format!("\n## {}\n\n", project.name));
        let checks = registry.latest_checks(&project.slug)?;
        if checks.is_empty() {
            out.push_str("Sin revisiones todavía.\n");
        }
        for c in checks {
            let mark = match c.status.as_str() {
                OK => "✅",
                WARN => "⚠️",
                _ => "❌",
            };
            out.push_str(&format!(
                "- {mark} **{}**: {} _(revisado {} UTC)_\n",
                c.kind, c.summary, c.checked_at
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_advisor_lints_by_level() {
        let body = json!({ "lints": [
            { "level": "ERROR", "title": "RLS desactivado" },
            { "level": "WARN", "title": "search_path mutable" },
            { "level": "WARN", "title": "otro" },
            { "level": "INFO", "title": "informativo" },
        ]});
        let (errors, warnings, listed) = count_lints(&body);
        assert_eq!((errors, warnings, listed.len()), (1, 2, 3));
        assert_eq!(count_lints(&json!([])).0, 0);
    }

    #[test]
    fn reads_major_version() {
        assert_eq!(major("16.4.6"), Some(16));
        assert_eq!(major("git+https://x"), None);
    }
}
