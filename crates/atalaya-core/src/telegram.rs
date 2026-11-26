//! Decisiones desde el teléfono: cada decisión nueva llega por Telegram con botones
//! (aprobar, dar por vista, descartar) y el Vigía escucha las respuestas.
//!
//! Solo se obedece al chat configurado en Conexiones → Avisos → Telegram; lo que
//! llegue de cualquier otro chat se ignora. Las respuestas se leen con `getUpdates`
//! (espera larga), sin abrir puertos ni webhooks.
//!
//! La lógica (qué mensaje mandar, qué hacer con cada botón) no toca la red, para
//! poder probarla; `listen` es la que habla con Telegram.

use crate::Registry;
use crate::decisions::{self, Decision};
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// "si" o "no": si las decisiones nuevas se mandan con botones. Por omisión, sí.
pub const ENABLED: &str = "telegram_decisions";
/// Desde cuándo se ofrecen: las pendientes de antes no llegan de golpe (para esas, /decisiones).
const SINCE: &str = "telegram_since";
/// El último `update_id` leído, para no procesar dos veces la misma respuesta.
const OFFSET: &str = "telegram_offset";
/// Cuántas decisiones nuevas se mandan por vuelta, para no inundar el chat.
const PER_ROUND: usize = 5;

pub(crate) fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS telegram_sent (
             decision_id INTEGER PRIMARY KEY,
             chat        TEXT NOT NULL,
             message_id  INTEGER NOT NULL,
             closed      INTEGER NOT NULL DEFAULT 0,
             sent_at     TEXT NOT NULL DEFAULT (datetime('now'))
         );",
    )?;
    Ok(())
}

/// Lo que hay que hacerle a Telegram como resultado de una respuesta o de una vuelta.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    /// Mensaje nuevo, con botones si `keyboard` no es nulo. `decision` anota a cuál corresponde.
    Send { text: String, keyboard: Value, decision: Option<i64> },
    /// Cambia el texto (y los botones) de un mensaje ya enviado.
    Edit { message_id: i64, text: String, keyboard: Value },
    /// Responde al toque de un botón: el aviso breve que ve quien lo tocó.
    Answer { callback_id: String, text: String },
    /// Pone al constructor a trabajar una mejora recién creada.
    Work { improvement: i64 },
}

/// El chat configurado, si Telegram está listo para decisiones.
pub fn chat(registry: &Registry) -> Result<Option<String>> {
    if registry.setting(ENABLED)?.as_deref() == Some("no") {
        return Ok(None);
    }
    Ok(registry.setting("telegram_chat")?.map(|c| c.trim().to_string()).filter(|c| !c.is_empty()))
}

/// Para la app: si hay chat configurado y si las decisiones van con botones.
pub fn status(registry: &Registry) -> Result<Value> {
    let configured = registry.setting("telegram_chat")?.is_some_and(|c| !c.trim().is_empty());
    Ok(json!({
        "configured": configured,
        "decisions": registry.setting(ENABLED)?.as_deref() != Some("no"),
    }))
}

pub fn set_enabled(registry: &Registry, on: bool) -> Result<()> {
    registry.set_setting(ENABLED, if on { "si" } else { "no" })
}

fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    format!("{}…", text.chars().take(max).collect::<String>().trim_end())
}

/// Si el agente puede trabajar en el proyecto (tiene carpeta en este Mac).
fn has_folder(registry: &Registry, slug: &str) -> bool {
    registry.get(slug).ok().flatten().and_then(|p| p.local_path).is_some()
}

/// Qué pasa al aprobar, como en la vista Decisiones.
fn effect(d: &Decision, can_work: bool) -> String {
    match d.action.as_str() {
        "mejora" => "Al aprobar se crea una mejora y el agente empieza a trabajarla.".into(),
        "comentario" => format!(
            "Al aprobar se publica la propuesta como comentario en el issue #{} de GitHub.",
            d.target.as_deref().unwrap_or("?")
        ),
        "reconocer" => format!(
            "Al aprobar se reconoce la alerta de {}: sigue vigilada, pero deja de contar como abierta.",
            d.target.as_deref().unwrap_or("?")
        ),
        "actualizar" => "Al aprobar, Atalaya trae lo último de la rama principal a tu carpeta local, solo si no pisa \
                         nada tuyo. El Vigía también lo reintenta solo."
            .into(),
        _ if can_work => "Al aprobar, el agente la toma como mejora y la trabaja en una rama aparte.".into(),
        _ => "El proyecto no tiene carpeta en el Mac: solo se puede dar por vista.".into(),
    }
}

/// El mensaje de una decisión pendiente.
pub fn message(registry: &Registry, d: &Decision) -> String {
    let name = registry.get(&d.slug).ok().flatten().map(|p| p.name).unwrap_or_else(|| d.slug.clone());
    let mut text = format!("Decisión #{} · {name}\n{}", d.id, d.title);
    if !d.context.is_empty() {
        text.push_str(&format!("\n\nSe observó: {}", clip(&d.context, 700)));
    }
    if !d.proposal.is_empty() {
        text.push_str(&format!("\n\nPropuesta: {}", clip(&d.proposal, 1200)));
    }
    text.push_str(&format!("\n\n{}", effect(d, has_folder(registry, &d.slug))));
    text
}

fn button(text: &str, data: String) -> Value {
    json!({ "text": text, "callback_data": data })
}

/// Los botones de una decisión pendiente. El dato de cada uno: `d:<id>:<a|v|x>`.
pub fn keyboard(registry: &Registry, d: &Decision) -> Value {
    let id = d.id;
    let row = if d.action == "ninguna" {
        if has_folder(registry, &d.slug) {
            vec![
                button("Que la trabaje el agente", format!("d:{id}:a")),
                button("Solo vista", format!("d:{id}:v")),
                button("Descartar", format!("d:{id}:x")),
            ]
        } else {
            vec![button("Dar por vista", format!("d:{id}:v")), button("Descartar", format!("d:{id}:x"))]
        }
    } else {
        let approve = if d.action == crate::local_sync::ACTION { "Poner al día" } else { "Aprobar" };
        vec![button(approve, format!("d:{id}:a")), button("Descartar", format!("d:{id}:x"))]
    };
    json!({ "inline_keyboard": [row] })
}

fn no_keyboard() -> Value {
    json!({ "inline_keyboard": [] })
}

/// Decisiones nuevas que todavía no llegaron al teléfono. La primera vez solo anota
/// desde cuándo, para no mandar de golpe las que ya estaban esperando.
pub fn offers(registry: &Registry) -> Result<Vec<Out>> {
    let since = match registry.setting(SINCE)? {
        Some(s) => s,
        None => {
            let now = registry.utc_now()?;
            registry.set_setting(SINCE, &now)?;
            now
        }
    };
    let mut stmt = registry.conn().prepare(
        "SELECT d.id FROM decisions d LEFT JOIN telegram_sent t ON t.decision_id = d.id
         WHERE d.status = 'pendiente' AND t.decision_id IS NULL AND d.created_at >= ?1
         ORDER BY d.id",
    )?;
    let ids: Vec<i64> = stmt.query_map([&since], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let pending = decisions::list(registry, true)?;
    Ok(ids
        .iter()
        .filter_map(|id| pending.iter().find(|d| d.id == *id))
        .take(PER_ROUND)
        .map(|d| Out::Send { text: message(registry, d), keyboard: keyboard(registry, d), decision: Some(d.id) })
        .collect())
}

/// Anota que la decisión ya llegó al teléfono, en ese mensaje.
pub fn mark_sent(registry: &Registry, decision: i64, chat: &str, message_id: i64) -> Result<()> {
    registry.conn().execute(
        "INSERT OR REPLACE INTO telegram_sent (decision_id, chat, message_id) VALUES (?1, ?2, ?3)",
        params![decision, chat, message_id],
    )?;
    Ok(())
}

/// Lo decidido en la app (o en la terminal) también se refleja en el teléfono: el mensaje
/// pierde sus botones y dice cómo terminó.
pub fn closures(registry: &Registry, chat: &str) -> Result<Vec<Out>> {
    let mut stmt = registry.conn().prepare(
        "SELECT t.message_id, d.id, d.title, d.status, coalesce(d.result, '')
         FROM telegram_sent t JOIN decisions d ON d.id = t.decision_id
         WHERE t.closed = 0 AND t.chat = ?1 AND d.status != 'pendiente'",
    )?;
    let rows: Vec<(i64, i64, String, String, String)> = stmt
        .query_map([chat], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut outs = Vec::new();
    for (message_id, id, title, status, result) in rows {
        registry.conn().execute("UPDATE telegram_sent SET closed = 1 WHERE decision_id = ?1", [id])?;
        outs.push(Out::Edit {
            message_id,
            text: format!(
                "Decisión #{id}: {title}\n\nResuelta en Atalaya: {}.",
                if result.is_empty() { capital(&status) } else { result }
            ),
            keyboard: no_keyboard(),
        });
    }
    Ok(outs)
}

fn capital(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// El chat de un mensaje o de un toque de botón, como texto.
fn chat_of(value: &Value) -> String {
    match &value["chat"]["id"] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

const HELP: &str = "Puedo mandarte las decisiones de Atalaya con botones para aprobarlas o descartarlas.\n\n\
/decisiones: las que esperan tu visto bueno\n/estado: cómo están los proyectos\n/ayuda: este mensaje";

/// Qué hacer con una actualización de Telegram. Lo que no venga del chat configurado se ignora.
pub fn handle(registry: &Registry, update: &Value, chat: &str) -> Result<Vec<Out>> {
    if let Some(query) = update.get("callback_query") {
        if chat_of(&query["message"]) != chat {
            return Ok(Vec::new());
        }
        let callback_id = query["id"].as_str().unwrap_or_default().to_string();
        let message_id = query["message"]["message_id"].as_i64().unwrap_or_default();
        let data = query["data"].as_str().unwrap_or_default();
        return Ok(press(registry, data, callback_id, message_id));
    }
    let Some(incoming) = update.get("message") else { return Ok(Vec::new()) };
    if chat_of(incoming) != chat {
        return Ok(Vec::new());
    }
    let text = incoming["text"].as_str().unwrap_or_default().trim();
    // "/decisiones@MiBot" en un grupo es lo mismo que "/decisiones".
    let command = text.split_whitespace().next().unwrap_or_default().split('@').next().unwrap_or_default();
    let reply = |text: String| Out::Send { text, keyboard: Value::Null, decision: None };
    Ok(match command.trim_start_matches('/').to_lowercase().as_str() {
        "decisiones" | "pendientes" => {
            let pending = decisions::list(registry, true)?;
            if pending.is_empty() {
                vec![reply("No hay decisiones pendientes.".into())]
            } else {
                let mut outs: Vec<Out> = pending
                    .iter()
                    .take(10)
                    .map(|d| Out::Send { text: message(registry, d), keyboard: keyboard(registry, d), decision: Some(d.id) })
                    .collect();
                if pending.len() > 10 {
                    outs.push(reply(format!("Y {} más: míralas en Atalaya → Decisiones.", pending.len() - 10)));
                }
                outs
            }
        }
        "estado" => vec![reply(clip(&crate::checks::report(registry)?, 3500))],
        _ => vec![reply(HELP.into())],
    })
}

/// Lo que hace cada botón.
fn press(registry: &Registry, data: &str, callback_id: String, message_id: i64) -> Vec<Out> {
    let answer = |text: &str| Out::Answer { callback_id: callback_id.clone(), text: clip(text, 190) };
    let parts: Vec<&str> = data.split(':').collect();
    let ["d", raw, choice] = parts.as_slice() else { return vec![answer("Ese botón ya no sirve.")] };
    let Ok(id) = raw.parse::<i64>() else { return vec![answer("Ese botón ya no sirve.")] };
    let Some(decision) = decisions::list(registry, false).ok().and_then(|all| all.into_iter().find(|d| d.id == id)) else {
        return vec![answer("Esa decisión ya no existe."), Out::Edit {
            message_id,
            text: format!("Decisión #{id}: ya no existe."),
            keyboard: no_keyboard(),
        }];
    };
    let header = format!("Decisión #{id}: {}", decision.title);
    if decision.status != "pendiente" {
        let _ = registry.conn().execute("UPDATE telegram_sent SET closed = 1 WHERE decision_id = ?1", [id]);
        return vec![
            answer(&format!("Ya estaba {}.", decision.status)),
            Out::Edit {
                message_id,
                text: format!("{header}\n\n{} antes. {}", capital(&decision.status), decision.result.unwrap_or_default()),
                keyboard: no_keyboard(),
            },
        ];
    }
    let outcome = match *choice {
        "x" => decisions::discard(registry, id).map(|_| (String::from("Descartada"), None)),
        "a" | "v" => {
            // Como en la app: aprobar una informativa con carpeta la convierte en trabajo.
            let work = *choice == "a" && decision.action == "ninguna" && has_folder(registry, &decision.slug);
            decisions::approve(registry, id, "", work).map(|improvement| {
                let result = decisions::list(registry, false)
                    .ok()
                    .and_then(|all| all.into_iter().find(|d| d.id == id))
                    .and_then(|d| d.result)
                    .unwrap_or_else(|| "Aprobada".into());
                (result, improvement)
            })
        }
        _ => return vec![answer("Ese botón ya no sirve.")],
    };
    match outcome {
        Ok((result, improvement)) => {
            let _ = registry.conn().execute("UPDATE telegram_sent SET closed = 1 WHERE decision_id = ?1", [id]);
            let mut outs = vec![answer(&result)];
            let mut text = format!("{header}\n\n{result}.");
            if let Some(n) = improvement {
                text.push_str(" El agente la empieza ahora; te aviso aquí cuando termine.");
                outs.push(Out::Work { improvement: n });
            }
            outs.insert(1, Out::Edit { message_id, text, keyboard: no_keyboard() });
            outs
        }
        // Sigue pendiente: los botones siguen sirviendo. Telegram corta el aviso a 200 caracteres.
        Err(e) => vec![answer(&clip(&format!("No se pudo: {e:#}"), 190))],
    }
}

/// Un solo trabajo del agente a la vez desde el teléfono; los demás quedan aceptados en Mejoras.
static WORKING: AtomicBool = AtomicBool::new(false);

fn api_base() -> String {
    std::env::var("ATALAYA_TELEGRAM_API").unwrap_or_else(|_| "https://api.telegram.org".into())
}

fn call(token: &str, method: &str, body: &Value, wait: u64) -> Result<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(wait + 15)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut response = agent
        .post(&format!("{}/bot{token}/{method}", api_base()))
        .header("Content-Type", "application/json")
        .send(body.to_string())
        .with_context(|| format!("no se pudo hablar con Telegram ({method})"))?;
    let reply: Value = serde_json::from_str(&response.body_mut().read_to_string().unwrap_or_default()).unwrap_or_default();
    if reply["ok"].as_bool() != Some(true) {
        bail!("Telegram respondió a {method}: {}", reply["description"].as_str().unwrap_or("sin detalle"));
    }
    Ok(reply["result"].clone())
}

/// Hace lo que pide cada `Out`. Un fallo en uno no frena los demás.
fn perform(registry: &Registry, token: &str, chat: &str, outs: Vec<Out>) {
    for out in outs {
        let done = match out {
            Out::Send { text, keyboard, decision } => {
                let mut body = json!({ "chat_id": chat, "text": text });
                if !keyboard.is_null() {
                    body["reply_markup"] = keyboard;
                }
                call(token, "sendMessage", &body, 0).and_then(|sent| match (decision, sent["message_id"].as_i64()) {
                    (Some(d), Some(m)) => mark_sent(registry, d, chat, m),
                    _ => Ok(()),
                })
            }
            Out::Edit { message_id, text, keyboard } => call(
                token,
                "editMessageText",
                &json!({ "chat_id": chat, "message_id": message_id, "text": text, "reply_markup": keyboard }),
                0,
            )
            .map(|_| ()),
            Out::Answer { callback_id, text } => {
                call(token, "answerCallbackQuery", &json!({ "callback_query_id": callback_id, "text": text }), 0).map(|_| ())
            }
            Out::Work { improvement } => {
                work(improvement);
                Ok(())
            }
        };
        if let Err(e) = done {
            eprintln!("telegram: {e:#}");
        }
    }
}

/// El constructor trabaja la mejora en otro hilo y avisa por Telegram cómo terminó.
fn work(improvement: i64) {
    if WORKING.swap(true, Ordering::SeqCst) {
        let _ = Registry::open_default().map(|r| {
            crate::checks::telegram(
                &r,
                &format!("La mejora #{improvement} quedó aceptada; el agente está ocupado con otra. Mándala desde Atalaya → Mejoras."),
            )
        });
        return;
    }
    std::thread::spawn(move || {
        let text = Registry::open_default().and_then(|r| {
            let done = crate::worker::run(&r, improvement, &|_, _| {})?;
            let mut text = format!("Mejora #{improvement}, {}: {}", done.title, done.status);
            if let Some(url) = done.pr_url {
                text.push_str(&format!("\n{url}"));
            }
            crate::checks::telegram(&r, &text);
            Ok(())
        });
        if let Err(e) = text {
            let _ = Registry::open_default()
                .map(|r| crate::checks::telegram(&r, &format!("La mejora #{improvement} no se pudo trabajar: {e:#}")));
        }
        WORKING.store(false, Ordering::SeqCst);
    });
}

/// Una vuelta: manda las decisiones nuevas, refleja las decididas en la app y espera
/// hasta `wait` segundos las respuestas. Devuelve `false` si Telegram no está configurado.
pub fn listen(registry: &Registry, wait: u64) -> Result<bool> {
    // El Llavero se consulta solo si hay chat: apagado no toca nada.
    let Some(chat) = chat(registry)? else { return Ok(false) };
    let Some(token) = crate::secrets::get("telegram") else { return Ok(false) };
    let mut outs = offers(registry)?;
    outs.extend(closures(registry, &chat)?);
    perform(registry, &token, &chat, outs);

    let offset = registry.setting(OFFSET)?.and_then(|o| o.parse::<i64>().ok()).unwrap_or(0);
    let updates = call(
        &token,
        "getUpdates",
        &json!({ "offset": offset + 1, "timeout": wait, "allowed_updates": ["message", "callback_query"] }),
        wait,
    )?;
    for update in updates.as_array().into_iter().flatten() {
        // Se anota antes de actuar: una respuesta que falla no se repite en cada vuelta.
        if let Some(id) = update["update_id"].as_i64() {
            registry.set_setting(OFFSET, &id.to_string())?;
        }
        match handle(registry, update, &chat) {
            Ok(outs) => perform(registry, &token, &chat, outs),
            Err(e) => eprintln!("telegram: {e:#}"),
        }
    }
    Ok(true)
}

/// El mensaje enviado para una decisión, si lo hay.
pub fn sent_message(registry: &Registry, decision: i64) -> Result<Option<i64>> {
    Ok(registry
        .conn()
        .query_row("SELECT message_id FROM telegram_sent WHERE decision_id = ?1", [decision], |r| r.get(0))
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Project;

    const CHAT: &str = "4242";

    fn registry(name: &str) -> (Registry, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("atalaya-tg-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("demo")).unwrap();
        let registry = Registry::open(&dir.join("p.db")).unwrap();
        registry
            .upsert(&Project {
                slug: "demo".into(),
                name: "Demo".into(),
                description: None,
                local_path: Some(dir.join("demo").to_string_lossy().into_owned()),
                repo: None,
                production_url: None,
                notes: None,
                vercel_project_id: None,
                vercel_team_id: None,
                supabase_ref: None,
                updated_at: String::new(),
            })
            .unwrap();
        registry.set_setting("telegram_chat", CHAT).unwrap();
        (registry, dir)
    }

    fn tap(id: i64, choice: &str, chat: &str) -> Value {
        json!({ "update_id": 1, "callback_query": {
            "id": "cb1", "data": format!("d:{id}:{choice}"),
            "message": { "message_id": 77, "chat": { "id": chat.parse::<i64>().unwrap() } }
        }})
    }

    #[test]
    fn a_new_decision_reaches_the_phone_once_with_buttons() {
        let (r, dir) = registry("ofrecer");
        // La primera vuelta solo anota desde cuándo: lo que ya esperaba no llega de golpe.
        assert!(offers(&r).unwrap().is_empty());
        r.conn().execute("UPDATE settings SET value = '2000-01-01 00:00:00' WHERE key = ?1", [SINCE]).unwrap();
        let id = decisions::propose(&r, "demo", "alerta", "Certificado por vencer", "Vence en 5 días", "Renovarlo", "ninguna", None)
            .unwrap();
        let outs = offers(&r).unwrap();
        let [Out::Send { text, keyboard, decision }] = outs.as_slice() else { panic!("{outs:?}") };
        assert_eq!(*decision, Some(id));
        assert!(text.contains("Demo") && text.contains("Vence en 5 días") && text.contains("Renovarlo"));
        let buttons: Vec<&str> =
            keyboard["inline_keyboard"][0].as_array().unwrap().iter().map(|b| b["callback_data"].as_str().unwrap()).collect();
        assert_eq!(buttons, [format!("d:{id}:a"), format!("d:{id}:v"), format!("d:{id}:x")]);

        mark_sent(&r, id, CHAT, 77).unwrap();
        assert!(offers(&r).unwrap().is_empty(), "no se repite");
        assert_eq!(sent_message(&r, id).unwrap(), Some(77));

        // Si se decide en la app, el mensaje pierde los botones una sola vez.
        decisions::discard(&r, id).unwrap();
        let outs = closures(&r, CHAT).unwrap();
        assert!(matches!(&outs[..], [Out::Edit { message_id: 77, text, .. }] if text.contains("Descartada")));
        assert!(closures(&r, CHAT).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_the_configured_chat_can_decide() {
        let (r, dir) = registry("chat");
        let id = decisions::propose(&r, "demo", "alerta", "Mirar", "", "", "ninguna", None).unwrap();
        assert!(handle(&r, &tap(id, "v", "999"), CHAT).unwrap().is_empty());
        assert_eq!(decisions::list(&r, true).unwrap().len(), 1, "otro chat no decide nada");

        let outs = handle(&r, &tap(id, "v", CHAT), CHAT).unwrap();
        assert!(matches!(&outs[0], Out::Answer { text, .. } if text == "Aprobada"), "{outs:?}");
        assert!(matches!(&outs[1], Out::Edit { message_id: 77, keyboard, .. } if keyboard["inline_keyboard"] == json!([])));
        assert!(decisions::list(&r, true).unwrap().is_empty());

        // Un segundo toque solo dice que ya estaba resuelta.
        let again = handle(&r, &tap(id, "x", CHAT), CHAT).unwrap();
        assert!(matches!(&again[0], Out::Answer { text, .. } if text.contains("aprobada")));
        assert_eq!(decisions::list(&r, false).unwrap()[0].status, "aprobada");

        let help = handle(&r, &json!({ "message": { "text": "hola", "chat": { "id": 999 } } }), CHAT).unwrap();
        assert!(help.is_empty(), "ni siquiera la ayuda a otro chat");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn approving_from_the_phone_can_put_the_agent_to_work() {
        let (r, dir) = registry("trabajo");
        let seen = decisions::propose(&r, "demo", "alerta", "Solo mirar", "", "Nada", "ninguna", None).unwrap();
        let work = decisions::propose(&r, "demo", "alerta", "Arreglar", "Se vio X", "Haz Y", "ninguna", None).unwrap();

        let outs = handle(&r, &tap(seen, "v", CHAT), CHAT).unwrap();
        assert!(!outs.iter().any(|o| matches!(o, Out::Work { .. })), "dar por vista no crea trabajo");
        assert!(r.improvements(None).unwrap().is_empty());

        let outs = handle(&r, &tap(work, "a", CHAT), CHAT).unwrap();
        let Some(Out::Work { improvement }) = outs.iter().find(|o| matches!(o, Out::Work { .. })) else { panic!("{outs:?}") };
        assert_eq!(r.improvement(*improvement).unwrap().status, "aceptada");

        // /decisiones: ya no queda ninguna.
        let list = handle(&r, &json!({ "message": { "text": "/decisiones@AtalayaBot", "chat": { "id": 4242 } } }), CHAT).unwrap();
        assert!(matches!(&list[..], [Out::Send { text, .. }] if text.contains("No hay")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn turned_off_it_does_not_listen() {
        let (r, dir) = registry("apagado");
        assert_eq!(chat(&r).unwrap().as_deref(), Some(CHAT));
        set_enabled(&r, false).unwrap();
        assert_eq!(chat(&r).unwrap(), None);
        assert!(!listen(&r, 0).unwrap(), "sin red ni Llavero");
        assert_eq!(status(&r).unwrap(), json!({ "configured": true, "decisions": false }));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
