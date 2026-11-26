//! Modelos que corren en este Mac: Ollama, LM Studio, Jan y llama.cpp.
//!
//! Todos hablan la API de OpenAI en una dirección local, sin llave ni costo por token.
//! Atalaya mira qué servidores responden y qué modelos tienen. Si el servidor está
//! apagado, muestra los modelos descargados y explica cómo encenderlo (LM Studio lo
//! enciende con `lms server start`).

use serde::Serialize;
use std::path::Path;

pub struct Server {
    pub id: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    /// La app de macOS, si tiene una.
    pub app: Option<&'static str>,
    /// El programa de terminal, si tiene uno.
    pub bin: Option<&'static str>,
    pub url: &'static str,
    /// Qué hacer para tener un modelo.
    pub hint: &'static str,
    /// Dónde guarda los modelos descargados (desde la carpeta personal) y a qué
    /// profundidad está cada modelo. Solo se leen los nombres de las carpetas.
    pub models_dir: Option<(&'static str, usize)>,
    /// Argumentos de `bin` que encienden su servidor, si se puede desde la terminal.
    pub start: Option<&'static [&'static str]>,
}

pub static SERVERS: &[Server] = &[
    Server {
        id: "ollama",
        label: "Ollama",
        base_url: "http://127.0.0.1:11434/v1",
        app: Some("Ollama"),
        bin: Some("ollama"),
        url: "https://ollama.com/download",
        hint: "Descarga un modelo desde la app de Ollama o con «ollama pull qwen3».",
        models_dir: None,
        start: None,
    },
    Server {
        id: "lmstudio",
        label: "LM Studio",
        base_url: "http://127.0.0.1:1234/v1",
        app: Some("LM Studio"),
        bin: Some("lms"),
        url: "https://lmstudio.ai",
        hint: "Descarga un modelo en LM Studio (Discover) y enciende su servidor con «Encender servidor» o en la pestaña Developer.",
        models_dir: Some((".lmstudio/models", 2)),
        start: Some(&["server", "start"]),
    },
    Server {
        id: "jan",
        label: "Jan",
        base_url: "http://127.0.0.1:1337/v1",
        app: Some("Jan"),
        bin: None,
        url: "https://jan.ai",
        hint: "En Jan, ve a Settings → Local API Server y pulsa Start Server (puerto 1337).",
        models_dir: Some(("Library/Application Support/Jan/data/llamacpp/models", 1)),
        start: None,
    },
    Server {
        id: "llamacpp",
        label: "llama.cpp",
        base_url: "http://127.0.0.1:8080/v1",
        app: None,
        bin: Some("llama-server"),
        url: "https://github.com/ggml-org/llama.cpp",
        hint: "Inicia «llama-server -hf» con el modelo que quieras.",
        models_dir: None,
        start: None,
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct Found {
    pub id: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    pub running: bool,
    pub models: Vec<String>,
    /// La app o el programa están instalados (aunque el servidor no responda).
    pub installed: bool,
    /// La app está abierta (aunque su servidor esté apagado).
    pub app_open: bool,
    /// Modelos descargados en el disco, aunque el servidor esté apagado.
    pub downloaded: Vec<String>,
    /// Atalaya puede encender su servidor.
    pub can_start: bool,
    pub app: Option<&'static str>,
    pub url: &'static str,
    pub hint: &'static str,
}

fn app_installed(name: &str) -> bool {
    let home = dirs::home_dir().unwrap_or_default();
    [Path::new("/Applications").join(format!("{name}.app")), home.join("Applications").join(format!("{name}.app"))]
        .iter()
        .any(|p| p.exists())
}

fn app_open(name: &str) -> bool {
    std::process::Command::new("/usr/bin/pgrep")
        .args(["-f", &format!("{name}.app/Contents/MacOS/")])
        .stdout(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn is_chat_model(name: &str) -> bool {
    let n = name.to_lowercase();
    !(n.contains("embed") || n.contains("sentence-transformer") || n.starts_with('.'))
}

/// Los nombres de las carpetas de modelos descargados.
fn downloaded(s: &Server) -> Vec<String> {
    let Some((rel, depth)) = s.models_dir else { return Vec::new() };
    let root = dirs::home_dir().unwrap_or_default().join(rel);
    let mut level = vec![(root, String::new())];
    for _ in 0..depth {
        let mut next = Vec::new();
        for (dir, prefix) in level {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if entry.path().is_dir() && !name.starts_with('.') {
                    let full = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
                    next.push((entry.path(), full));
                }
            }
        }
        level = next;
    }
    let mut names: Vec<String> = level
        .into_iter()
        .filter(|(_, n)| is_chat_model(n))
        .map(|(dir, n)| {
            // LM Studio deja «.part» o «downloading_…» mientras baja el modelo.
            let partial = std::fs::read_dir(&dir).into_iter().flatten().flatten().any(|e| {
                let f = e.file_name().to_string_lossy().to_string();
                f.ends_with(".part") || f.starts_with("downloading_")
            });
            if partial { format!("{n} (descargando)") } else { n }
        })
        .collect();
    names.sort();
    names
}

/// La misma dirección con `127.0.0.1` en vez de `localhost`. Node (Qwen Code, Gemini CLI…)
/// prueba primero `::1`, y Ollama y LM Studio solo escuchan en `127.0.0.1`.
pub fn loopback(url: &str) -> String {
    url.replacen("://localhost", "://127.0.0.1", 1)
}

/// Los modelos de un servidor compatible con OpenAI, o `None` si no responde.
pub fn models(base_url: &str) -> Option<Vec<String>> {
    let (code, body) = crate::providers::http("GET", &format!("{}/models", base_url.trim_end_matches('/')), &[], None, 2).ok()?;
    if !(200..300).contains(&code) {
        return None;
    }
    let mut list: Vec<String> = body["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["id"].as_str().map(String::from))
        // Los modelos de embeddings no conversan.
        .filter(|id| !id.to_lowercase().contains("embed"))
        .collect();
    list.sort();
    Some(list)
}

fn ollama_models() -> Option<Vec<String>> {
    let (code, body) = crate::providers::http("GET", "http://127.0.0.1:11434/api/tags", &[], None, 2).ok()?;
    (200..300).contains(&code).then(|| {
        body["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(String::from)).collect()
    })
}

fn check(s: &'static Server) -> Found {
    let mut models = models(s.base_url);
    if s.id == "ollama" && models.as_ref().is_none_or(Vec::is_empty) {
        models = ollama_models().or(models);
    }
    Found {
        id: s.id,
        label: s.label,
        base_url: s.base_url,
        running: models.is_some(),
        models: models.unwrap_or_default(),
        installed: s.app.is_some_and(app_installed) || s.bin.is_some_and(|b| crate::agents::find_program(b).is_some()),
        app_open: s.app.is_some_and(app_open),
        downloaded: downloaded(s),
        can_start: s.start.is_some() && s.bin.is_some_and(|b| crate::agents::find_program(b).is_some()),
        app: s.app,
        url: s.url,
        hint: s.hint,
    }
}

/// Mira todos los servidores locales a la vez.
pub fn detect() -> Vec<Found> {
    let handles: Vec<_> = SERVERS.iter().map(|s| std::thread::spawn(move || check(s))).collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}

/// Abre la app de un servidor local que está instalado pero cerrado.
pub fn open_app(id: &str) -> anyhow::Result<()> {
    let app = SERVERS.iter().find(|s| s.id == id).and_then(|s| s.app).ok_or_else(|| anyhow::anyhow!("'{id}' no tiene app"))?;
    let status = std::process::Command::new("/usr/bin/open").args(["-a", app]).status()?;
    anyhow::ensure!(status.success(), "no se pudo abrir {app}");
    Ok(())
}

/// Enciende el servidor local (LM Studio con `lms server start`) y espera a que responda.
pub fn start_server(id: &str) -> anyhow::Result<()> {
    let s = SERVERS.iter().find(|s| s.id == id).ok_or_else(|| anyhow::anyhow!("servidor desconocido '{id}'"))?;
    let (Some(bin), Some(args)) = (s.bin, s.start) else {
        anyhow::bail!("{} no se puede encender desde Atalaya. {}", s.label, s.hint);
    };
    let program = crate::agents::find_program(bin).ok_or_else(|| anyhow::anyhow!("no se encontró «{bin}»"))?;
    let out = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).output()?;
    for _ in 0..20 {
        if models(s.base_url).is_some() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    anyhow::bail!("{} no encendió su servidor: {}", s.label, text.trim())
}

/// Agrega un modelo local como conexión (sin llave). Si ya hay una con la misma dirección
/// y modelo, devuelve esa.
pub fn add(registry: &crate::Registry, id: &str, model: &str) -> anyhow::Result<String> {
    let s = SERVERS.iter().find(|s| s.id == id).ok_or_else(|| anyhow::anyhow!("servidor desconocido '{id}'"))?;
    for c in crate::connections::of_provider(registry, "ollama")? {
        if c.config_str("base_url").as_deref() == Some(s.base_url) && c.config_str("model").as_deref() == Some(model) {
            return Ok(c.id);
        }
    }
    let c = crate::connections::save(
        registry,
        &crate::connections::Input {
            id: None,
            provider: "ollama".into(),
            name: format!("{} · {model}", s.label),
            config: serde_json::json!({ "base_url": s.base_url, "model": model }),
            secret: None,
        },
    )?;
    Ok(c.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_downloaded_models_by_folder_name() {
        let home = dirs::home_dir().unwrap();
        let rel = format!(".cache/atalaya-modelos-{}", std::process::id());
        let root = home.join(&rel);
        std::fs::create_dir_all(root.join("editor/listo")).unwrap();
        std::fs::create_dir_all(root.join("editor/bajando")).unwrap();
        std::fs::create_dir_all(root.join("editor/nomic-embed")).unwrap();
        std::fs::write(root.join("editor/bajando/model.safetensors.part"), "").unwrap();
        let leaked: &'static str = Box::leak(rel.into_boxed_str());
        let s = Server { models_dir: Some((leaked, 2)), ..SERVERS[1] };
        assert_eq!(downloaded(&s), vec!["editor/bajando (descargando)".to_string(), "editor/listo".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_models_from_a_local_server() {
        let base = crate::testutil::models_server(r#"{"data":[{"id":"qwen3:8b"},{"id":"nomic-embed-text"},{"id":"llama3.2"}]}"#);
        assert_eq!(models(&base).unwrap(), vec!["llama3.2".to_string(), "qwen3:8b".to_string()]);
        assert!(models("http://127.0.0.1:9/v1").is_none(), "un servidor apagado no tiene modelos");
    }
}
