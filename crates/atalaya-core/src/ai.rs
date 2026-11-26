//! Modelos de IA por API: Anthropic, OpenAI, Gemini, cualquiera compatible con OpenAI
//! (OpenRouter, DeepSeek, Groq…) y los locales sin llave (Ollama, LM Studio).
//!
//! Un solo cliente (`complete`) habla los tres formatos y anota cada llamada en
//! `ai_usage`: tokens de entrada y salida y, si se conoce el precio, su costo. Los
//! precios viven en `ai_prices`, editables. Los modelos locales cuestan 0; OpenRouter
//! informa sus precios; para el resto el precio lo pone la persona. Atalaya nunca
//! inventa un precio: sin precio, el costo queda sin definir y se muestran los tokens.

use crate::Registry;
use crate::connections::Connection;
use crate::providers::{expect_2xx, http};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Formato de la API que habla cada proveedor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Anthropic,
    OpenAi,
    Gemini,
}

pub fn format_of(provider: &str) -> Option<Format> {
    match provider {
        "anthropic" => Some(Format::Anthropic),
        "openai" | "openai-compatible" | "ollama" => Some(Format::OpenAi),
        "gemini" => Some(Format::Gemini),
        _ => None,
    }
}

pub(crate) fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS ai_usage (
             id            INTEGER PRIMARY KEY AUTOINCREMENT,
             at            TEXT NOT NULL DEFAULT (datetime('now')),
             connection_id TEXT NOT NULL,
             provider      TEXT NOT NULL,
             model         TEXT NOT NULL,
             task          TEXT NOT NULL,
             slug          TEXT,
             input_tokens  INTEGER,
             output_tokens INTEGER,
             cost_usd      REAL,
             ok            INTEGER NOT NULL,
             error         TEXT
         );
         CREATE INDEX IF NOT EXISTS ai_usage_at ON ai_usage (at);
         CREATE TABLE IF NOT EXISTS ai_prices (
             provider          TEXT NOT NULL,
             model             TEXT NOT NULL,
             input_per_mtok    REAL NOT NULL,
             output_per_mtok   REAL NOT NULL,
             source            TEXT NOT NULL,
             updated_at        TEXT NOT NULL DEFAULT (datetime('now')),
             PRIMARY KEY (provider, model)
         );",
    )?;
    Ok(())
}

/// Dirección base de la API de una conexión, sin la barra final.
pub fn base_url(c: &Connection) -> Result<String> {
    let url = match c.provider.as_str() {
        "anthropic" => "https://api.anthropic.com/v1".to_string(),
        "openai" => "https://api.openai.com/v1".to_string(),
        "gemini" => "https://generativelanguage.googleapis.com/v1beta".to_string(),
        _ => c.config_str("base_url").context("la conexión no tiene dirección de API")?,
    };
    Ok(url.trim_end_matches('/').to_string())
}

/// Un modelo que corre en el propio equipo o la red local: no cuesta por token.
pub fn is_local(c: &Connection) -> bool {
    c.provider == "ollama"
        || base_url(c).is_ok_and(|u| {
            let host = crate::providers::host_of(&u);
            let host = host.split(':').next().unwrap_or_default();
            host == "localhost" || host == "127.0.0.1" || host.ends_with(".local") || host.starts_with("192.168.")
        })
}

fn is_openrouter(c: &Connection) -> bool {
    base_url(c).is_ok_and(|u| crate::providers::host_of(&u) == "openrouter.ai")
}

/// Nombre con que se guardan los precios: el proveedor, o el servicio si es compatible.
fn price_provider(c: &Connection) -> String {
    match c.provider.as_str() {
        "openai-compatible" => base_url(c).map(|u| crate::providers::host_of(&u)).unwrap_or_else(|_| c.provider.clone()),
        other => other.to_string(),
    }
}

/// Cabeceras de autenticación según el formato.
pub(crate) fn auth_headers(c: &Connection) -> Result<Vec<(&'static str, String)>> {
    let secret = c.secret();
    Ok(match format_of(&c.provider) {
        Some(Format::Anthropic) => vec![
            ("x-api-key", secret.context("falta la llave de Anthropic")?),
            ("anthropic-version", "2023-06-01".to_string()),
        ],
        Some(Format::Gemini) => vec![("x-goog-api-key", secret.context("falta la llave de Gemini")?)],
        Some(Format::OpenAi) => match secret {
            Some(s) => vec![("Authorization", format!("Bearer {s}"))],
            None if c.provider == "openai" => bail!("falta la llave de OpenAI"),
            None => vec![],
        },
        None => bail!("'{}' no es un proveedor de IA", c.provider),
    })
}

/// Modelos que ofrece la conexión. En OpenRouter, guarda además sus precios.
pub fn models(registry: &Registry, c: &Connection) -> Result<Vec<String>> {
    let base = base_url(c)?;
    let headers = auth_headers(c)?;
    let service = c.name.as_str();
    let mut names: Vec<String> = match format_of(&c.provider) {
        Some(Format::Gemini) => {
            let body = expect_2xx(service, http("GET", &format!("{base}/models?pageSize=200"), &headers, None, 30)?)?;
            body["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|m| {
                    m["supportedGenerationMethods"]
                        .as_array()
                        .is_none_or(|ms| ms.iter().any(|x| x == "generateContent"))
                })
                .filter_map(|m| m["name"].as_str().map(|n| n.trim_start_matches("models/").to_string()))
                .collect()
        }
        // Cloudflare no lista modelos en su API compatible: se piden a la suya, solo los de texto.
        _ if crate::connections::preset_of(&base).is_some_and(|p| p.id == "cloudflare") => {
            let search = format!("{}/models/search?task=Text%20Generation&per_page=100", base.trim_end_matches("/v1"));
            let body = expect_2xx(service, http("GET", &search, &headers, None, 30)?)?;
            body["result"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(str::to_string)).collect()
        }
        _ => {
            let body = expect_2xx(service, http("GET", &format!("{base}/models"), &headers, None, 30)?)?;
            let list = body["data"].as_array().or(body["models"].as_array()).cloned().unwrap_or_default();
            if is_openrouter(c) {
                save_openrouter_prices(registry, c, &list)?;
            }
            list.iter().filter_map(|m| m["id"].as_str().or(m["name"].as_str()).map(str::to_string)).collect()
        }
    };
    names.sort();
    names.dedup();
    Ok(names)
}

/// OpenRouter publica el precio por token de cada modelo.
fn save_openrouter_prices(registry: &Registry, c: &Connection, list: &[Value]) -> Result<()> {
    let provider = price_provider(c);
    let per_token = |v: &Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64());
    for m in list {
        let (Some(id), Some(input), Some(output)) =
            (m["id"].as_str(), per_token(&m["pricing"]["prompt"]), per_token(&m["pricing"]["completion"]))
        else {
            continue;
        };
        // Un precio negativo es la marca de OpenRouter para "variable": no se guarda.
        if input < 0.0 || output < 0.0 {
            continue;
        }
        let existing: Option<String> = registry
            .conn()
            .query_row("SELECT source FROM ai_prices WHERE provider = ?1 AND model = ?2", params![provider, id], |r| r.get(0))
            .optional()?;
        // Lo que la persona fijó a mano manda sobre lo informado.
        if existing.as_deref() == Some("manual") {
            continue;
        }
        registry.conn().execute(
            "INSERT INTO ai_prices (provider, model, input_per_mtok, output_per_mtok, source) VALUES (?1, ?2, ?3, ?4, 'openrouter')
             ON CONFLICT(provider, model) DO UPDATE SET input_per_mtok = excluded.input_per_mtok,
                 output_per_mtok = excluded.output_per_mtok, source = 'openrouter', updated_at = datetime('now')",
            params![provider, id, input * 1e6, output * 1e6],
        )?;
    }
    Ok(())
}

/// Prueba una conexión de IA listando sus modelos.
pub fn test(registry: &Registry, c: &Connection) -> Result<String> {
    let names = models(registry, c)?;
    if names.is_empty() {
        return Ok("Conectado, pero no informa modelos".to_string());
    }
    let sample: Vec<&str> = names.iter().map(String::as_str).take(4).collect();
    // Un modelo mal escrito (o una llave pegada donde va el modelo) se nota al guardar.
    if let Some(model) = c.config_str("model") {
        if !names.iter().any(|n| *n == model || n.ends_with(&format!("/{model}"))) {
            bail!(
                "conectado, pero el modelo elegido no está en este servicio. Tiene: {}{}. Si es una llave, va en el campo de la llave",
                sample.join(", "),
                if names.len() > 4 { "…" } else { "" }
            );
        }
    }
    Ok(format!(
        "Conectado: {} modelos ({}{})",
        names.len(),
        sample.join(", "),
        if names.len() > 4 { "…" } else { "" }
    ))
}

/* ------------------------------------------------------------------ Llamadas */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// `user` o `assistant`.
    pub role: String,
    pub content: String,
    /// Imágenes que acompañan al texto (adjuntos del chat).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<crate::attachments::Image>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Self { role: "user".into(), content: text.into(), images: Vec::new() }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self { role: "assistant".into(), content: text.into(), images: Vec::new() }
    }

    /// El contenido en el formato de cada proveedor: texto solo, o imágenes y texto.
    fn content_for(&self, format: Format) -> Value {
        if self.images.is_empty() {
            return json!(self.content);
        }
        let mut parts: Vec<Value> = self
            .images
            .iter()
            .map(|i| match format {
                Format::Anthropic => {
                    json!({ "type": "image", "source": { "type": "base64", "media_type": i.mime, "data": i.data } })
                }
                Format::OpenAi => {
                    json!({ "type": "image_url", "image_url": { "url": format!("data:{};base64,{}", i.mime, i.data) } })
                }
                Format::Gemini => json!({ "inline_data": { "mime_type": i.mime, "data": i.data } }),
            })
            .collect();
        parts.push(match format {
            Format::Gemini => json!({ "text": self.content }),
            _ => json!({ "type": "text", "text": self.content }),
        });
        Value::Array(parts)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Request {
    /// Modelo; vacío usa el predeterminado de la conexión.
    pub model: Option<String>,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub max_tokens: u32,
    /// Para qué es la llamada: revisión, validación, chat… Va al registro de consumo.
    pub task: String,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub text: String,
    pub model: String,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// `None` cuando no se conoce el precio del modelo.
    pub cost_usd: Option<f64>,
}

/// El cuerpo y la ruta de una llamada en el formato del proveedor.
fn build(format: Format, provider: &str, model: &str, req: &Request) -> (String, Value) {
    let max = if req.max_tokens == 0 { 4096 } else { req.max_tokens };
    match format {
        Format::Anthropic => {
            let mut body = json!({
                "model": model,
                "max_tokens": max,
                "messages": req.messages.iter().map(|m| json!({ "role": m.role, "content": m.content_for(format) })).collect::<Vec<_>>(),
            });
            if let Some(system) = &req.system {
                body["system"] = json!(system);
            }
            ("/messages".to_string(), body)
        }
        Format::OpenAi => {
            let mut messages = Vec::new();
            if let Some(system) = &req.system {
                messages.push(json!({ "role": "system", "content": system }));
            }
            messages.extend(req.messages.iter().map(|m| json!({ "role": m.role, "content": m.content_for(format) })));
            let mut body = json!({ "model": model, "messages": messages });
            // OpenAI cambió el nombre del límite; los compatibles siguen con el de antes.
            let limit = if provider == "openai" { "max_completion_tokens" } else { "max_tokens" };
            body[limit] = json!(max);
            ("/chat/completions".to_string(), body)
        }
        Format::Gemini => {
            let contents: Vec<Value> = req
                .messages
                .iter()
                .map(|m| {
                    let role = if m.role == "assistant" { "model" } else { "user" };
                    let parts = match m.content_for(format) {
                        Value::Array(parts) => parts,
                        _ => vec![json!({ "text": m.content })],
                    };
                    json!({ "role": role, "parts": parts })
                })
                .collect();
            let mut body = json!({ "contents": contents, "generationConfig": { "maxOutputTokens": max } });
            if let Some(system) = &req.system {
                body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
            }
            (format!("/models/{model}:generateContent"), body)
        }
    }
}

/// Texto y tokens de una respuesta, en el formato del proveedor. El costo informado por
/// el servicio (OpenRouter lo manda en `usage.cost`) viene aparte.
fn parse(format: Format, body: &Value) -> Result<(String, Option<u64>, Option<u64>, Option<f64>)> {
    Ok(match format {
        Format::Anthropic => {
            let text = body["content"]
                .as_array()
                .context("la respuesta no trae contenido")?
                .iter()
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            (text, body["usage"]["input_tokens"].as_u64(), body["usage"]["output_tokens"].as_u64(), None)
        }
        Format::OpenAi => {
            let text = body["choices"][0]["message"]["content"].as_str().context("la respuesta no trae texto")?.to_string();
            (
                text,
                body["usage"]["prompt_tokens"].as_u64(),
                body["usage"]["completion_tokens"].as_u64(),
                body["usage"]["cost"].as_f64(),
            )
        }
        Format::Gemini => {
            let text = body["candidates"][0]["content"]["parts"]
                .as_array()
                .context("la respuesta no trae texto")?
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("");
            (
                text,
                body["usageMetadata"]["promptTokenCount"].as_u64(),
                body["usageMetadata"]["candidatesTokenCount"].as_u64(),
                None,
            )
        }
    })
}

/// Precio por millón de tokens (entrada, salida) de un modelo, si se conoce.
pub fn price(registry: &Registry, c: &Connection, model: &str) -> Result<Option<(f64, f64)>> {
    if is_local(c) {
        return Ok(Some((0.0, 0.0)));
    }
    Ok(registry
        .conn()
        .query_row(
            "SELECT input_per_mtok, output_per_mtok FROM ai_prices WHERE provider = ?1 AND model = ?2",
            params![price_provider(c), model],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

fn cost(price: Option<(f64, f64)>, input: Option<u64>, output: Option<u64>) -> Option<f64> {
    let (pi, po) = price?;
    Some((input? as f64 * pi + output? as f64 * po) / 1e6)
}

/// Anota una llamada en el consumo. Los agentes de terminal también la usan.
pub fn record(
    registry: &Registry,
    c: &Connection,
    model: &str,
    task: &str,
    slug: Option<&str>,
    tokens: (Option<u64>, Option<u64>),
    cost_usd: Option<f64>,
    error: Option<&str>,
) -> Result<()> {
    insert(registry, &c.id, &c.provider, model, task, slug, tokens, cost_usd, error)
}

/// Con qué id queda Claude Code en el consumo: no es una conexión, es el de este Mac.
pub const CLAUDE_CODE_ID: &str = "claude-code";

/// Modelo y tokens del resultado final de Claude Code (`--output-format json` o el evento
/// `result` de `stream-json`). La entrada suma lo que leyó de la caché, porque también lo procesa.
pub fn claude_code_usage(result: &Value) -> (String, Option<u64>, Option<u64>) {
    let usage = &result["usage"];
    let input = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"]
        .iter()
        .filter_map(|k| usage[*k].as_u64())
        .reduce(|a, b| a + b);
    let output = usage["output_tokens"].as_u64();
    // Con subagentes usa varios modelos: se anota el que más trabajó.
    let cost = |v: &Value| v["costUSD"].as_f64().unwrap_or(0.0);
    let model = result["modelUsage"]
        .as_object()
        .and_then(|m| m.iter().max_by(|a, b| cost(a.1).total_cmp(&cost(b.1))).map(|(k, _)| k.clone()))
        .unwrap_or_else(|| CLAUDE_CODE_ID.to_string());
    (model, input, output)
}

/// Anota un trabajo de Claude Code. Lo cubre la cuenta de Claude de la persona, así que no
/// lleva costo propio; sin resultado (se cortó antes) quedan sin tokens.
pub fn record_claude_code(registry: &Registry, task: &str, slug: Option<&str>, result: Option<&Value>, error: Option<&str>) -> Result<()> {
    let (model, input, output) = result.map(claude_code_usage).unwrap_or_else(|| (CLAUDE_CODE_ID.to_string(), None, None));
    insert(registry, CLAUDE_CODE_ID, CLAUDE_CODE_ID, &model, task, slug, (input, output), None, error)
}

/// Corre un trabajo de Claude Code y lo anota, salga bien o mal. `work` deja en su
/// argumento el resultado final de Claude Code cuando lo recibe.
pub fn tracked_claude<T>(
    registry: &Registry,
    task: &str,
    slug: Option<&str>,
    work: impl FnOnce(&mut Option<Value>) -> Result<T>,
) -> Result<T> {
    let mut last = None;
    let result = work(&mut last);
    let error = result.as_ref().err().map(|e| format!("{e:#}"));
    record_claude_code(registry, task, slug, last.as_ref(), error.as_deref())?;
    result
}

#[allow(clippy::too_many_arguments)]
fn insert(
    registry: &Registry,
    connection_id: &str,
    provider: &str,
    model: &str,
    task: &str,
    slug: Option<&str>,
    tokens: (Option<u64>, Option<u64>),
    cost_usd: Option<f64>,
    error: Option<&str>,
) -> Result<()> {
    registry.conn().execute(
        "INSERT INTO ai_usage (connection_id, provider, model, task, slug, input_tokens, output_tokens, cost_usd, ok, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            connection_id,
            provider,
            model,
            if task.is_empty() { "otra" } else { task },
            slug,
            tokens.0.map(|t| t as i64),
            tokens.1.map(|t| t as i64),
            cost_usd,
            error.is_none(),
            error,
        ],
    )?;
    // Sin cuota, la IA queda en pausa hasta que se renueve; una llamada buena la saca.
    let name = if connection_id == CLAUDE_CODE_ID {
        "Claude Code".to_string()
    } else {
        crate::connections::get(registry, connection_id)?.map(|c| c.name).unwrap_or_else(|| connection_id.to_string())
    };
    crate::quota::note(registry, connection_id, &name, error)?;
    Ok(())
}

/// Llama a un modelo y anota la llamada en el consumo, salga bien o mal.
pub fn complete(registry: &Registry, c: &Connection, req: &Request) -> Result<Reply> {
    let format = format_of(&c.provider).with_context(|| format!("«{}» no es una conexión de IA por API", c.name))?;
    let model = req
        .model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .or_else(|| c.config_str("model"))
        .with_context(|| format!("elige un modelo para «{}» (en la conexión o en el rol)", c.name))?;
    let result = (|| -> Result<Reply> {
        let mut req = req.clone();
        req.max_tokens = fit_cap(registry, c, &model, &req)?;
        let (path, mut body) = build(format, &c.provider, &model, &req);
        if is_openrouter(c) {
            body["usage"] = json!({ "include": true });
        }
        let response = http("POST", &format!("{}{path}", base_url(c)?), &auth_headers(c)?, Some(&body), 300)?;
        let body = expect_2xx(&c.name, response)?;
        let (text, input, output, reported) = parse(format, &body)?;
        let cost_usd = reported.or(cost(price(registry, c, &model)?, input, output));
        Ok(Reply { text, model: model.clone(), input_tokens: input, output_tokens: output, cost_usd })
    })();
    let slug = req.slug.as_deref();
    match &result {
        Ok(r) => record(registry, c, &model, &req.task, slug, (r.input_tokens, r.output_tokens), r.cost_usd, None)?,
        Err(e) => record(registry, c, &model, &req.task, slug, (None, None), None, Some(&format!("{e:#}")))?,
    }
    result
}

/* ------------------------------------------------------------------- Consumo */

/// Fija a mano el precio de un modelo (dólares por millón de tokens).
pub fn set_price(registry: &Registry, connection_id: &str, model: &str, input: f64, output: f64) -> Result<()> {
    let c = crate::connections::get(registry, connection_id)?.with_context(|| format!("no existe la conexión '{connection_id}'"))?;
    if !(input.is_finite() && output.is_finite()) || input < 0.0 || output < 0.0 {
        bail!("los precios deben ser números positivos");
    }
    let model = model.trim();
    if model.is_empty() {
        bail!("falta el modelo");
    }
    registry.conn().execute(
        "INSERT INTO ai_prices (provider, model, input_per_mtok, output_per_mtok, source) VALUES (?1, ?2, ?3, ?4, 'manual')
         ON CONFLICT(provider, model) DO UPDATE SET input_per_mtok = excluded.input_per_mtok,
             output_per_mtok = excluded.output_per_mtok, source = 'manual', updated_at = datetime('now')",
        params![price_provider(&c), model, input, output],
    )?;
    // Las llamadas del mes que quedaron sin costo se calculan con el precio nuevo.
    registry.conn().execute(
        "UPDATE ai_usage SET cost_usd = (coalesce(input_tokens, 0) * ?1 + coalesce(output_tokens, 0) * ?2) / 1e6
         WHERE connection_id = ?3 AND model = ?4 AND cost_usd IS NULL AND input_tokens IS NOT NULL
           AND at >= date('now', 'start of month')",
        params![input, output, connection_id, model],
    )?;
    Ok(())
}

fn budget_key(connection_id: &str) -> String {
    format!("ai_budget:{connection_id}")
}

/// Presupuesto mensual de una conexión, en dólares. `None` o 0 lo quita.
pub fn set_budget(registry: &Registry, connection_id: &str, usd: Option<f64>) -> Result<()> {
    let value = usd.filter(|u| u.is_finite() && *u > 0.0).map(|u| u.to_string()).unwrap_or_default();
    registry.set_setting(&budget_key(connection_id), &value)
}

pub fn budget(registry: &Registry, connection_id: &str) -> Result<Option<f64>> {
    Ok(registry.setting(&budget_key(connection_id))?.and_then(|v| v.parse().ok()).filter(|v: &f64| *v > 0.0))
}

/// Gasto del mes en curso de una conexión (solo lo que tiene precio).
pub fn month_spent(registry: &Registry, connection_id: &str) -> Result<f64> {
    Ok(registry.conn().query_row(
        "SELECT coalesce(sum(cost_usd), 0) FROM ai_usage WHERE connection_id = ?1 AND at >= date('now', 'start of month')",
        [connection_id],
        |r| r.get(0),
    )?)
}

/* --------------------------------------------------------- Tope por encargo */

/// Cuánto puede gastar un solo encargo (un turno del chat, una mejora, una consulta…).
pub const TASK_CAP: &str = "ai_task_cap_usd";

/// Fija el tope por encargo en dólares. `None` o 0 lo quita.
pub fn set_task_cap(registry: &Registry, usd: Option<f64>) -> Result<()> {
    if usd.is_some_and(|u| !u.is_finite() || u < 0.0) {
        bail!("el tope debe ser un número positivo");
    }
    let value = usd.filter(|u| *u > 0.0).map(|u| u.to_string()).unwrap_or_default();
    registry.set_setting(TASK_CAP, &value)
}

pub fn task_cap(registry: &Registry) -> Result<Option<f64>> {
    Ok(registry.setting(TASK_CAP)?.and_then(|v| v.parse().ok()).filter(|v: &f64| *v > 0.0))
}

/// Lo que se le agrega a Claude Code para que respete el tope (`--max-budget-usd`). Claude
/// lo mira entre un paso y otro, así que un paso largo puede pasarlo un poco.
pub fn claude_cap_args(registry: &Registry) -> Result<Vec<String>> {
    Ok(match task_cap(registry)? {
        Some(cap) => vec!["--max-budget-usd".into(), format!("{cap}")],
        None => Vec::new(),
    })
}

/// Si Claude Code se detuvo por llegar al tope, el aviso para la persona.
pub fn claude_cap_note(result: &Value) -> Option<String> {
    if result["subtype"] != "error_max_budget_usd" {
        return None;
    }
    let spent = result["total_cost_usd"].as_f64().map(|c| format!(" (iba en US$ {c:.2})")).unwrap_or_default();
    Some(format!(
        "Se detuvo al llegar al tope por encargo{spent}. Súbelo en Consumo → Tope por encargo si necesita más."
    ))
}

/// Cuántos tokens de respuesta caben en el tope, según el precio del modelo. Sin tope o sin
/// precio conocido no limita (no se inventan precios); si solo leer ya cuesta más, no llama.
fn fit_cap(registry: &Registry, c: &Connection, model: &str, req: &Request) -> Result<u32> {
    let max = if req.max_tokens == 0 { 4096 } else { req.max_tokens };
    let Some(cap) = task_cap(registry)? else { return Ok(max) };
    let Some((pi, po)) = price(registry, c, model)? else { return Ok(max) };
    // Por lo alto: unos 3 caracteres por token y 1.600 tokens por imagen.
    let chars = req.system.as_deref().map_or(0, |s| s.chars().count())
        + req.messages.iter().map(|m| m.content.chars().count()).sum::<usize>();
    let images: usize = req.messages.iter().map(|m| m.images.len()).sum();
    let input = (chars / 3 + images * 1_600) as f64;
    let reading = input * pi / 1e6;
    if reading >= cap {
        bail!("este encargo costaría unos US$ {reading:.3} solo en leer, más que el tope de US$ {cap:.2} por encargo");
    }
    if po <= 0.0 {
        return Ok(max);
    }
    let fits = ((cap - reading) * 1e6 / po).floor();
    if fits < 256.0 {
        bail!("con el tope de US$ {cap:.2} por encargo casi no queda para responder; súbelo en Consumo");
    }
    Ok(max.min(fits.min(u32::MAX as f64) as u32))
}

/// Si la conexión ya pasó su presupuesto del mes.
pub fn over_budget(registry: &Registry, connection_id: &str) -> Result<bool> {
    Ok(match budget(registry, connection_id)? {
        Some(cap) => month_spent(registry, connection_id)? >= cap,
        None => false,
    })
}

/// Resumen del mes: por conexión y modelo, por tarea, presupuestos y precios.
pub fn usage_summary(registry: &Registry) -> Result<Value> {
    let conn = registry.conn();
    let all = crate::connections::all(registry)?;
    let mut names: std::collections::HashMap<String, (String, String)> =
        all.iter().map(|c| (c.id.clone(), (c.name.clone(), c.provider.clone()))).collect();
    names.insert(CLAUDE_CODE_ID.into(), ("Claude Code".into(), CLAUDE_CODE_ID.into()));
    // Cómo se paga cada una: los agentes y Claude Code van con la cuenta de la persona, no
    // por token, así que nunca quedan como «precio sin definir».
    let billing = |id: &str| -> &'static str {
        if id == CLAUDE_CODE_ID {
            return "cuenta";
        }
        match all.iter().find(|c| c.id == id) {
            Some(c) if crate::orchestrator::is_free(&crate::orchestrator::Candidate::Connection(c.clone())) => "gratis",
            Some(c) if c.kind == "agente" => "cuenta",
            _ => "api",
        }
    };
    let mut stmt = conn.prepare(
        "SELECT connection_id, model, count(*), sum(ok = 0), coalesce(sum(input_tokens), 0), coalesce(sum(output_tokens), 0),
                sum(cost_usd), sum(cost_usd IS NULL AND ok = 1)
         FROM ai_usage WHERE at >= date('now', 'start of month')
         GROUP BY connection_id, model ORDER BY sum(cost_usd) DESC, count(*) DESC",
    )?;
    let by_model: Vec<Value> = stmt
        .query_map([], |r| {
            let id: String = r.get(0)?;
            let (name, provider) = names.get(&id).cloned().unwrap_or((id.clone(), String::new()));
            let paid_by = billing(&id);
            Ok(json!({
                "connection_id": id,
                "connection": name,
                "provider": provider,
                "model": r.get::<_, String>(1)?,
                "calls": r.get::<_, i64>(2)?,
                "errors": r.get::<_, i64>(3)?,
                "input_tokens": r.get::<_, i64>(4)?,
                "output_tokens": r.get::<_, i64>(5)?,
                "cost_usd": r.get::<_, Option<f64>>(6)?,
                "unpriced_calls": if paid_by == "api" { r.get::<_, i64>(7)? } else { 0 },
                "billing": paid_by,
            }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let by_ai = share_by_ai(registry, &names, &billing)?;
    let mut stmt = conn.prepare(
        "SELECT task, count(*), coalesce(sum(input_tokens), 0) + coalesce(sum(output_tokens), 0), sum(cost_usd)
         FROM ai_usage WHERE at >= date('now', 'start of month') GROUP BY task ORDER BY count(*) DESC",
    )?;
    let by_task: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "task": r.get::<_, String>(0)?,
                "calls": r.get::<_, i64>(1)?,
                "tokens": r.get::<_, i64>(2)?,
                "cost_usd": r.get::<_, Option<f64>>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut connections = Vec::new();
    for c in all.iter().filter(|c| c.kind == "ia" && c.provider != "anthropic-admin" || c.kind == "agente") {
        connections.push(json!({
            "id": c.id,
            "name": c.name,
            "kind": c.kind,
            "provider": c.provider,
            "model": c.config_str("model"),
            "local": format_of(&c.provider).is_some() && is_local(c),
            "budget_usd": budget(registry, &c.id)?,
            "spent_usd": month_spent(registry, &c.id)?,
        }));
    }
    let mut stmt = conn.prepare(
        "SELECT provider, model, input_per_mtok, output_per_mtok, source, updated_at FROM ai_prices
         WHERE source = 'manual' OR model IN (SELECT DISTINCT model FROM ai_usage) ORDER BY provider, model",
    )?;
    let prices: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "provider": r.get::<_, String>(0)?,
                "model": r.get::<_, String>(1)?,
                "input_per_mtok": r.get::<_, f64>(2)?,
                "output_per_mtok": r.get::<_, f64>(3)?,
                "source": r.get::<_, String>(4)?,
                "updated_at": r.get::<_, String>(5)?,
            }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let total: f64 = connections.iter().filter_map(|c| c["spent_usd"].as_f64()).fold(0.0, |a, b| a + b);
    Ok(json!({
        "month_usd": total,
        "by_ai": by_ai,
        "by_model": by_model,
        "by_task": by_task,
        "connections": connections,
        "prices": prices,
        "task_cap_usd": task_cap(registry)?,
    }))
}

/// Quién hizo el trabajo del mes: cada IA con su parte de las llamadas y de los tokens,
/// sus tareas y cómo se paga. Cuenta también lo gratis y lo que falló.
fn share_by_ai(
    registry: &Registry,
    names: &std::collections::HashMap<String, (String, String)>,
    billing: &dyn Fn(&str) -> &'static str,
) -> Result<Value> {
    let conn = registry.conn();
    let mut stmt = conn.prepare(
        "SELECT connection_id, task, count(*), sum(ok = 0), coalesce(sum(input_tokens), 0) + coalesce(sum(output_tokens), 0),
                sum(input_tokens IS NULL AND output_tokens IS NULL), max(at)
         FROM ai_usage WHERE at >= date('now', 'start of month') GROUP BY connection_id, task",
    )?;
    let rows: Vec<(String, String, i64, i64, i64, i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let calls: i64 = rows.iter().map(|r| r.2).sum();
    let tokens: i64 = rows.iter().map(|r| r.4).sum();
    let percent = |part: i64, whole: i64| if whole > 0 { (part as f64 * 1000.0 / whole as f64).round() / 10.0 } else { 0.0 };
    let ids: std::collections::BTreeSet<&String> = rows.iter().map(|r| &r.0).collect();
    // Por qué falló la última vez y si está en pausa por falta de cuota.
    let mut last_error = conn.prepare(
        "SELECT error, at FROM ai_usage WHERE connection_id = ?1 AND ok = 0 AND at >= date('now', 'start of month')
         ORDER BY id DESC LIMIT 1",
    )?;
    let mut status = std::collections::HashMap::new();
    for id in &ids {
        let error: Option<(Option<String>, String)> =
            last_error.query_row([id.as_str()], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        let paused = match crate::quota::paused(registry, id)? {
            Some(p) => Some(json!({ "until": crate::quota::local_time(registry, p.until)?, "reason": p.reason })),
            None => None,
        };
        let error = error.map(|(text, at)| json!({ "text": text, "at": at }));
        status.insert((*id).clone(), (error, paused));
    }
    let mut list: Vec<Value> = ids
        .into_iter()
        .map(|id| {
            let (last_error, paused) = status.remove(id).unwrap_or_default();
            let mine: Vec<_> = rows.iter().filter(|r| &r.0 == id).collect();
            let n: i64 = mine.iter().map(|r| r.2).sum();
            let t: i64 = mine.iter().map(|r| r.4).sum();
            let mut tasks: Vec<Value> = mine.iter().map(|r| json!({ "task": r.1, "calls": r.2 })).collect();
            tasks.sort_by_key(|v| std::cmp::Reverse(v["calls"].as_i64()));
            let (name, provider) = names.get(id).cloned().unwrap_or((id.clone(), String::new()));
            json!({
                "id": id,
                "name": name,
                "provider": provider,
                "billing": billing(id),
                "calls": n,
                "errors": mine.iter().map(|r| r.3).sum::<i64>(),
                "calls_pct": percent(n, calls),
                "tokens": t,
                "tokens_pct": percent(t, tokens),
                // Llamadas que no informaron tokens (muchos agentes no lo dicen).
                "untracked_calls": mine.iter().map(|r| r.5).sum::<i64>(),
                "tasks": tasks,
                "last_at": mine.iter().map(|r| r.6.as_str()).max(),
                "last_error": last_error,
                "paused": paused,
            })
        })
        .collect();
    list.sort_by_key(|v| std::cmp::Reverse(v["calls"].as_i64()));
    let free: i64 = list.iter().filter(|v| v["billing"] == "gratis").filter_map(|v| v["calls"].as_i64()).sum();
    Ok(json!({ "calls": calls, "tokens": tokens, "free_pct": percent(free, calls), "ais": list }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// Servidor HTTP mínimo que responde como Anthropic, OpenAI o Gemini según la ruta.
    /// Con el modelo `falla` responde un error.
    pub(crate) fn fake_server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let sent: Value = serde_json::from_slice(&body).unwrap_or_default();
                let path = first.split_whitespace().nth(1).unwrap_or_default().to_string();
                let failing = sent["model"] == "falla" || path.contains("/falla:");
                let reply = if failing {
                    json!({ "error": { "message": "modelo inexistente" } })
                } else if path.ends_with("/messages") {
                    json!({ "content": [{ "type": "text", "text": format!("claude: {}", sent["messages"][0]["content"].as_str().unwrap_or_default()) }], "usage": { "input_tokens": 1000, "output_tokens": 500 } })
                } else if path.ends_with("/chat/completions") {
                    let last = sent["messages"].as_array().and_then(|m| m.last()).cloned().unwrap_or_default();
                    json!({ "choices": [{ "message": { "content": format!("openai: {}", last["content"].as_str().unwrap_or_default()) } }], "usage": { "prompt_tokens": 2000, "completion_tokens": 1000 } })
                } else if path.contains(":generateContent") {
                    json!({ "candidates": [{ "content": { "parts": [{ "text": "gemini: hola" }] } }], "usageMetadata": { "promptTokenCount": 10, "candidatesTokenCount": 5 } })
                } else {
                    json!({ "data": [{ "id": "modelo-a" }, { "id": "modelo-b" }] })
                };
                let status = if failing { "400 Bad Request" } else { "200 OK" };
                let text = reply.to_string();
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                    text.len()
                );
            }
        });
        format!("http://{addr}")
    }

    pub(crate) fn temp_registry(name: &str) -> (Registry, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("atalaya-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (Registry::open(&dir.join("atalaya.db")).unwrap(), dir)
    }

    pub(crate) fn local_ai(registry: &Registry, name: &str, base: &str, model: &str) -> Connection {
        crate::connections::save(
            registry,
            &crate::connections::Input {
                id: None,
                provider: "ollama".into(),
                name: name.into(),
                config: json!({ "base_url": base, "model": model }),
                secret: None,
            },
        )
        .unwrap()
    }

    #[test]
    fn the_task_cap_trims_the_answer_or_skips_the_call() {
        let (r, dir) = temp_registry("tope-api");
        // Uno de pago (los locales cuestan 0 y no se limitan), sin guardarlo: no toca el Llavero.
        let c = Connection {
            id: "pagado".into(),
            kind: "ia".into(),
            provider: "anthropic".into(),
            name: "Pagado".into(),
            config: json!({ "model": "caro" }),
            secret_account: None,
            created_at: String::new(),
        };
        r.conn()
            .execute("INSERT INTO ai_prices (provider, model, input_per_mtok, output_per_mtok, source) VALUES ('anthropic', 'caro', 10, 100, 'manual')", [])
            .unwrap();
        let req = |chars: usize| Request {
            messages: vec![Message::user("x".repeat(chars))],
            max_tokens: 4096,
            task: "chat".into(),
            ..Default::default()
        };
        // Sin tope, lo pedido.
        assert_eq!(fit_cap(&r, &c, "caro", &req(300)).unwrap(), 4096);
        // US$ 0,05: leer 100 tokens cuesta US$ 0,001; quedan 490 tokens de respuesta.
        set_task_cap(&r, Some(0.05)).unwrap();
        assert_eq!(fit_cap(&r, &c, "caro", &req(300)).unwrap(), 490);
        // Solo leer ya pasa el tope: no llama, y queda anotado como error.
        let err = complete(&r, &c, &req(30_000)).unwrap_err().to_string();
        assert!(err.contains("más que el tope"), "{err}");
        let failed: i64 = r.conn().query_row("SELECT count(*) FROM ai_usage WHERE ok = 0 AND task = 'chat'", [], |x| x.get(0)).unwrap();
        assert_eq!(failed, 1);
        // Un modelo sin precio conocido no se limita: no se inventan precios.
        assert_eq!(fit_cap(&r, &c, "otro", &req(300)).unwrap(), 4096);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn images_go_inside_the_message_in_each_format() {
        let mut msg = Message::user("¿qué falla aquí?");
        msg.images.push(crate::attachments::Image { mime: "image/png".into(), data: "QUJD".into() });
        let req = Request { messages: vec![msg], ..Default::default() };
        let (_, body) = build(Format::Anthropic, "anthropic", "m", &req);
        let content = &body["messages"][0]["content"];
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[0]["source"]["data"], "QUJD");
        assert_eq!(content[1]["text"], "¿qué falla aquí?");
        let (_, body) = build(Format::OpenAi, "openai", "m", &req);
        assert_eq!(body["messages"][0]["content"][0]["image_url"]["url"], "data:image/png;base64,QUJD");
        assert_eq!(body["messages"][0]["content"][1]["type"], "text");
        let (_, body) = build(Format::Gemini, "gemini", "m", &req);
        assert_eq!(body["contents"][0]["parts"][0]["inline_data"]["mime_type"], "image/png");
        assert_eq!(body["contents"][0]["parts"][1]["text"], "¿qué falla aquí?");
        // Sin imágenes, el contenido sigue siendo texto simple.
        let (_, body) = build(Format::Anthropic, "anthropic", "m", &Request { messages: vec![Message::user("hola")], ..Default::default() });
        assert_eq!(body["messages"][0]["content"], "hola");
    }

    #[test]
    fn builds_and_parses_each_format() {
        let req = Request { system: Some("sé breve".into()), messages: vec![Message::user("hola")], max_tokens: 100, ..Default::default() };
        let (path, body) = build(Format::Anthropic, "anthropic", "m", &req);
        assert_eq!(path, "/messages");
        assert_eq!(body["system"], "sé breve");
        let (_, body) = build(Format::OpenAi, "openai", "m", &req);
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["max_completion_tokens"], 100);
        let (_, body) = build(Format::OpenAi, "openai-compatible", "m", &req);
        assert_eq!(body["max_tokens"], 100);
        let (path, body) = build(Format::Gemini, "gemini", "m", &req);
        assert_eq!(path, "/models/m:generateContent");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sé breve");
        let (text, i, o, _) = parse(
            Format::Anthropic,
            &json!({ "content": [{ "text": "a" }, { "text": "b" }], "usage": { "input_tokens": 3, "output_tokens": 4 } }),
        )
        .unwrap();
        assert_eq!((text.as_str(), i, o), ("ab", Some(3), Some(4)));
        assert_eq!(cost(None, Some(10), Some(10)), None, "sin precio no hay costo");
        assert_eq!(cost(Some((3.0, 15.0)), Some(1_000_000), Some(100_000)), Some(4.5));
    }

    /// Probar una conexión avisa si el modelo elegido no está en el servicio.
    #[test]
    fn testing_catches_a_model_that_is_not_there() {
        let base = fake_server();
        let (registry, dir) = temp_registry("ia-modelo");
        let good = local_ai(&registry, "Bien", &base, "modelo-a");
        assert!(test(&registry, &good).unwrap().contains("2 modelos"));
        let wrong = local_ai(&registry, "Mal", &base, "d56d98ea-0000-0000-0000-000000000000");
        let err = test(&registry, &wrong).unwrap_err().to_string();
        assert!(err.contains("no está en este servicio") && err.contains("modelo-a"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn calls_are_recorded_with_cost_only_when_the_price_is_known() {
        let base = fake_server();
        let (registry, dir) = temp_registry("ia");
        let local = local_ai(&registry, "Local", &base, "modelo-a");
        let req = Request { messages: vec![Message::user("hola")], task: "prueba".into(), ..Default::default() };
        let reply = complete(&registry, &local, &req).unwrap();
        assert_eq!(reply.text, "openai: hola");
        assert_eq!(reply.cost_usd, Some(0.0), "lo local cuesta cero");

        // Una conexión remota sin precio conocido no inventa costo.
        let mut away = local.clone();
        away.provider = "openai-compatible".into();
        away.config["base_url"] = json!("https://api.ejemplo.invalid/v1");
        assert!(!is_local(&away));
        assert_eq!(price(&registry, &away, "modelo-a").unwrap(), None);

        // Una llamada fallida también queda anotada.
        let bad = Request { model: Some("falla".into()), ..req.clone() };
        assert!(complete(&registry, &local, &bad).is_err());
        let summary = usage_summary(&registry).unwrap();
        let rows = summary["by_model"].as_array().unwrap();
        assert_eq!(rows.iter().map(|r| r["calls"].as_i64().unwrap()).sum::<i64>(), 2);
        assert_eq!(rows.iter().map(|r| r["errors"].as_i64().unwrap()).sum::<i64>(), 1);
        assert_eq!(summary["by_task"][0]["task"], "prueba");

        set_budget(&registry, &local.id, Some(1.0)).unwrap();
        assert_eq!(budget(&registry, &local.id).unwrap(), Some(1.0));
        assert!(!over_budget(&registry, &local.id).unwrap());
        set_budget(&registry, &local.id, None).unwrap();
        assert_eq!(budget(&registry, &local.id).unwrap(), None);
        assert!(set_price(&registry, &local.id, "modelo-a", -1.0, 2.0).is_err());
        set_price(&registry, &local.id, "modelo-a", 1.0, 2.0).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn claude_code_counts_in_the_share_of_work() {
        let base = fake_server();
        let (registry, dir) = temp_registry("ia-reparto");
        // Lo que entrega Claude Code al terminar: tokens, caché y el modelo que más trabajó.
        let result = json!({
            "type": "result",
            "usage": { "input_tokens": 10, "cache_creation_input_tokens": 100, "cache_read_input_tokens": 1000, "output_tokens": 50 },
            "modelUsage": { "claude-haiku-4-5": { "costUSD": 0.001 }, "claude-opus-5-5": { "costUSD": 0.2 } },
        });
        assert_eq!(claude_code_usage(&result), ("claude-opus-5-5".to_string(), Some(1110), Some(50)));
        assert_eq!(claude_code_usage(&json!({})), (CLAUDE_CODE_ID.to_string(), None, None));

        for _ in 0..3 {
            tracked_claude(&registry, "chat", None, |last| {
                *last = Some(result.clone());
                Ok(())
            })
            .unwrap();
        }
        // Uno que falla antes de responder también cuenta, sin tokens.
        let failed: Result<()> = tracked_claude(&registry, "ronda", None, |_| bail!("sin sesión"));
        assert!(failed.is_err());
        let local = local_ai(&registry, "Local", &base, "modelo-a");
        let req = Request { messages: vec![Message::user("hola")], task: "chat".into(), ..Default::default() };
        complete(&registry, &local, &req).unwrap();

        let summary = usage_summary(&registry).unwrap();
        let share = &summary["by_ai"];
        assert_eq!(share["calls"], 5);
        assert_eq!(share["free_pct"], 20.0, "solo la local es gratis");
        let ais = share["ais"].as_array().unwrap();
        assert_eq!(ais[0]["name"], "Claude Code");
        assert_eq!(ais[0]["billing"], "cuenta");
        assert_eq!(ais[0]["calls_pct"], 80.0);
        assert_eq!(ais[0]["errors"], 1);
        assert_eq!(ais[0]["untracked_calls"], 1);
        assert_eq!(ais[0]["tasks"][0]["task"], "chat");
        assert_eq!(ais[1]["billing"], "gratis");
        // Con la cuenta no hay «precio sin definir».
        let claude_rows: Vec<_> = summary["by_model"].as_array().unwrap().iter().filter(|r| r["connection_id"] == CLAUDE_CODE_ID).collect();
        assert!(claude_rows.iter().all(|r| r["unpriced_calls"] == 0 && r["connection"] == "Claude Code"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn talks_to_anthropic_and_gemini_formats() {
        let base = fake_server();
        let (registry, dir) = temp_registry("ia2");
        let req = Request { messages: vec![Message::user("hola")], task: "prueba".into(), ..Default::default() };
        for (format, path, expected) in [
            (Format::Anthropic, "/messages", "claude: hola"),
            (Format::Gemini, "/models/m:generateContent", "gemini: hola"),
        ] {
            let (_, body) = build(format, "x", "m", &req);
            let (_, answer) = http("POST", &format!("{base}{path}"), &[], Some(&body), 10).unwrap();
            assert_eq!(parse(format, &answer).unwrap().0, expected);
        }
        let local = local_ai(&registry, "Ollama", &base, "modelo-a");
        assert!(is_local(&local));
        assert_eq!(models(&registry, &local).unwrap(), vec!["modelo-a", "modelo-b"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
