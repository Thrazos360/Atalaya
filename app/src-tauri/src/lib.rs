//! Puente entre la ventana y el núcleo de Atalaya.

mod chat;
mod tray;
mod work;

use atalaya_core::{Project, Registry, flags};
use serde_json::Value;

fn text(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
async fn list_projects() -> Result<Vec<Project>, String> {
    Registry::open_default().and_then(|r| r.list()).map_err(text)
}

#[tauri::command]
async fn project_context(slug: String) -> Result<Value, String> {
    Registry::open_default()
        .and_then(|r| atalaya_core::project_context(&r, &slug))
        .map_err(text)
}

/// Contexto de todos los proyectos de una vez, para la consola.
#[tauri::command]
async fn overview() -> Result<Vec<Value>, String> {
    tauri::async_runtime::spawn_blocking(|| -> anyhow::Result<Vec<Value>> {
        let registry = Registry::open_default()?;
        registry
            .list()?
            .iter()
            .map(|p| atalaya_core::project_context(&registry, &p.slug))
            .collect()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

fn with_registry<T>(task: impl FnOnce(&Registry) -> anyhow::Result<T>) -> Result<T, String> {
    Registry::open_default().and_then(|r| task(&r)).map_err(text)
}

/// Reconoce (o vuelve a abrir) la alerta de una revisión.
#[tauri::command]
async fn ack_alert(slug: String, kind: String, acked: bool) -> Result<(), String> {
    with_registry(|r| {
        if acked { r.acknowledge(&slug, &kind) } else { r.unacknowledge(&slug, &kind) }
    })
}

/// Envía una alerta a resolver: crea su mejora y pone al agente a trabajarla.
#[tauri::command]
async fn resolve_alert(app: tauri::AppHandle, slug: String, kind: String) -> Result<i64, String> {
    let id = with_registry(|r| atalaya_core::worker::resolve_alert(r, &slug, &kind))?;
    work::start_improvement(app, id).await?;
    Ok(id)
}

/// Activa o desactiva que las mejoras se fusionen solas cuando pasan sus comprobaciones.
#[tauri::command]
async fn set_auto_merge(on: bool) -> Result<(), String> {
    with_registry(|r| r.set_setting(atalaya_core::worker::AUTO_MERGE, if on { "si" } else { "no" }))
}

#[tauri::command]
async fn list_improvements() -> Result<Vec<atalaya_core::Improvement>, String> {
    with_registry(|r| r.improvements(None))
}

#[tauri::command]
async fn add_improvement(slug: String, title: String, detail: String) -> Result<(), String> {
    with_registry(|r| r.add_improvement(&slug, &title, &detail, "app").map(|_| ()))
}

#[tauri::command]
async fn set_improvement_status(id: i64, status: String) -> Result<(), String> {
    with_registry(|r| r.set_improvement_status(id, &status))
}

#[tauri::command]
async fn delete_improvement(id: i64) -> Result<(), String> {
    with_registry(|r| r.delete_improvement(id))
}

/// Repos de GitHub de la cuenta. Habla con GitHub, así que corre fuera del hilo de la interfaz.
#[tauri::command]
async fn github_repos() -> Result<Vec<atalaya_core::github::Repo>, String> {
    tauri::async_runtime::spawn_blocking(atalaya_core::github::repos)
        .await
        .map_err(|e| e.to_string())?
        .map_err(text)
}

#[tauri::command]
async fn github_create(name: String, desc: String, private: bool) -> Result<atalaya_core::github::Repo, String> {
    tauri::async_runtime::spawn_blocking(move || atalaya_core::github::create(&name, &desc, private))
        .await
        .map_err(|e| e.to_string())?
        .map_err(text)
}

/// Empieza a vigilar un repo de GitHub que no tiene carpeta en esta máquina.
#[tauri::command]
async fn github_watch(repo: String, url: Option<String>) -> Result<(), String> {
    with_registry(|r| atalaya_core::github::watch(r, &repo, None, url).map(|_| ()))
}

/// Quita un proyecto de la vigilancia. No toca su carpeta ni su repo.
#[tauri::command]
async fn remove_project(slug: String) -> Result<(), String> {
    with_registry(|r| r.remove(&slug).map(|_| ()))
}

/// Comprobación del sistema. Lee la carpeta de cada proyecto, así que macOS muestra de
/// una vez los avisos de permiso que falten.
#[tauri::command]
async fn doctor() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| -> anyhow::Result<Value> {
        atalaya_core::status::doctor(&Registry::open_default()?)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

#[tauri::command]
async fn list_decisions() -> Result<Value, String> {
    with_registry(|r| Ok(serde_json::json!({
        "decisions": atalaya_core::decisions::list(r, false)?,
        "round": atalaya_core::round::last(r)?,
        "telegram": atalaya_core::telegram::status(r)?,
    })))
}

/// El historial del chat (ver `conversations.rs`): se guarda tras cada turno.
#[tauri::command]
async fn conversations_save(
    id: String,
    slug: Option<String>,
    model: String,
    messages: Vec<atalaya_core::conversations::Message>,
) -> Result<(), String> {
    with_registry(|r| atalaya_core::conversations::save(r, &id, slug.as_deref(), &model, &messages))
}

#[tauri::command]
async fn conversations_list() -> Result<Vec<atalaya_core::conversations::Summary>, String> {
    with_registry(|r| atalaya_core::conversations::list(r, 60))
}

#[tauri::command]
async fn conversations_search(query: String, slug: Option<String>) -> Result<Vec<atalaya_core::conversations::Hit>, String> {
    with_registry(|r| atalaya_core::conversations::search(r, &query, slug.as_deref(), 60))
}

#[tauri::command]
async fn conversations_get(id: String) -> Result<atalaya_core::conversations::Conversation, String> {
    with_registry(|r| atalaya_core::conversations::get(r, &id))
}

/// Lo conversado, para que la IA lo retome.
#[tauri::command]
async fn conversations_recap(id: String) -> Result<String, String> {
    with_registry(|r| atalaya_core::conversations::recap(r, &id, 12_000))
}

/// Exporta a Markdown en la ruta que eligió el usuario con el diálogo de guardar.
#[tauri::command]
async fn conversations_export(id: String, path: String) -> Result<(), String> {
    with_registry(|r| atalaya_core::conversations::export(r, &id, std::path::Path::new(&path)))
}

/// Si las decisiones nuevas llegan a Telegram con botones.
#[tauri::command]
async fn telegram_decisions(on: bool) -> Result<(), String> {
    with_registry(|r| atalaya_core::telegram::set_enabled(r, on))
}

/// Aprueba una decisión con el texto final. Si crea una mejora, la pone a trabajar.
/// Con `work`, una decisión informativa también se convierte en mejora.
#[tauri::command]
async fn approve_decision(app: tauri::AppHandle, id: i64, proposal: String, work: bool) -> Result<(), String> {
    let improvement = tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<Option<i64>> {
        atalaya_core::decisions::approve(&Registry::open_default()?, id, &proposal, work)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)?;
    if let Some(improvement) = improvement {
        work::start_improvement(app, improvement).await?;
    }
    Ok(())
}

#[tauri::command]
async fn discard_decision(id: i64) -> Result<(), String> {
    with_registry(|r| atalaya_core::decisions::discard(r, id))
}

/// Pide una ronda al agente ahora. Tarda uno o dos minutos.
#[tauri::command]
async fn run_round() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| -> anyhow::Result<String> {
        atalaya_core::round::run(&Registry::open_default()?, &|_, _| {})
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

#[tauri::command]
async fn radar() -> Result<Value, String> {
    with_registry(|r| Ok(serde_json::json!({
        "competitors": atalaya_core::radar::competitors(r)?,
        "events": atalaya_core::radar::events(r)?,
    })))
}

#[tauri::command]
async fn radar_add(slug: String, name: String, url: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
        let registry = Registry::open_default()?;
        let id = atalaya_core::radar::add(&registry, &slug, &name, &url)?;
        // La primera lectura deja la referencia; si falla, queda anotado en el competidor.
        let _ = atalaya_core::radar::check(&registry, id);
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

#[tauri::command]
async fn radar_remove(id: i64) -> Result<(), String> {
    with_registry(|r| atalaya_core::radar::remove(r, id))
}

#[tauri::command]
async fn radar_check() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(|| -> anyhow::Result<()> {
        let registry = Registry::open_default()?;
        for c in atalaya_core::radar::competitors(&registry)? {
            let _ = atalaya_core::radar::check(&registry, c.id);
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

/// Historia del sitio por día: disponibilidad y tiempo de respuesta.
#[tauri::command]
async fn site_history(slug: String) -> Result<Vec<Value>, String> {
    with_registry(|r| {
        Ok(r.site_history(&slug)?
            .into_iter()
            .map(|(day, total, ok, ms)| serde_json::json!({ "day": day, "total": total, "ok": ok, "ms": ms }))
            .collect())
    })
}

/// Clona el repo de un proyecto vigilado solo en remoto.
#[tauri::command]
async fn clone_project(slug: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<String> {
        let registry = Registry::open_default()?;
        Ok(atalaya_core::github::clone(&registry, &slug)?.display().to_string())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

/// Respalda ahora las bases de un proyecto.
#[tauri::command]
async fn backup_project(slug: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<()> {
        let registry = Registry::open_default()?;
        let project = registry.get(&slug)?.ok_or_else(|| anyhow::anyhow!("no existe el proyecto"))?;
        if atalaya_core::backup::backup_project(&registry, &project)?.is_none() {
            anyhow::bail!("este proyecto no tiene una base de datos conocida y con llave");
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

/// Consumo de Claude Platform: lo último leído, sin tocar la red.
#[tauri::command]
async fn claude_status() -> Result<Value, String> {
    with_registry(atalaya_core::claude::status)
}

/* ------------------------------------------------------------- Conexiones */

/* ------------------------------------------------------------------ Skills e instrucciones */

/// Las skills instaladas, las que ya tienes en tus agentes y las instrucciones comunes.
#[tauri::command]
async fn skills_list() -> Result<Value, String> {
    blocking(|r| {
        let mut projects = serde_json::Map::new();
        for p in r.list()? {
            let text = atalaya_core::skills::instructions(r, Some(&p.slug))?;
            if !text.is_empty() {
                projects.insert(p.slug, Value::String(text));
            }
        }
        Ok(serde_json::json!({
            "skills": atalaya_core::skills::list(r)?,
            "found": atalaya_core::skills::found_elsewhere(r)?,
            "instructions": { "general": atalaya_core::skills::instructions(r, None)?, "projects": projects },
        }))
    })
    .await
}

/// Instala las skills de una carpeta de tu Mac (o importa una de tus agentes).
#[tauri::command]
async fn skills_install_folder(path: String, source: Option<String>) -> Result<Vec<atalaya_core::skills::Skill>, String> {
    blocking(move |r| atalaya_core::skills::install_folder(r, std::path::Path::new(&path), source.as_deref().unwrap_or(&path))).await
}

/// Instala las skills de un repo de git.
#[tauri::command]
async fn skills_install_git(url: String) -> Result<Vec<atalaya_core::skills::Skill>, String> {
    blocking(move |r| atalaya_core::skills::install_git(r, &url)).await
}

#[tauri::command]
async fn skills_review(name: String) -> Result<atalaya_core::skills::Review, String> {
    blocking(move |r| atalaya_core::skills::review(r, &name)).await
}

#[tauri::command]
async fn skills_configure(
    name: String,
    enabled: bool,
    projects: Option<Vec<String>>,
    agents: Option<Vec<String>>,
) -> Result<atalaya_core::skills::Skill, String> {
    blocking(move |r| atalaya_core::skills::configure(r, &name, enabled, projects, agents)).await
}

/// Quita una skill: su carpeta va a la Papelera.
#[tauri::command]
async fn skills_remove(name: String) -> Result<(), String> {
    blocking(move |r| atalaya_core::skills::remove(r, &name)).await
}

/* ------------------------------------------------------------------ Memoria y comandos guardados */

/// La memoria propuesta y la aprobada.
#[tauri::command]
async fn memories_list() -> Result<Vec<atalaya_core::memories::Memory>, String> {
    blocking(atalaya_core::memories::list).await
}

/// Aprueba (con el texto ajustado, si viene) o descarta una memoria.
#[tauri::command]
async fn memories_decide(id: i64, approve: bool, text: Option<String>) -> Result<atalaya_core::memories::Memory, String> {
    blocking(move |r| atalaya_core::memories::decide(r, id, approve, text.as_deref())).await
}

/// Una memoria escrita por el usuario (queda aprobada).
#[tauri::command]
async fn memories_add(slug: Option<String>, text: String) -> Result<atalaya_core::memories::Memory, String> {
    blocking(move |r| atalaya_core::memories::add(r, slug.as_deref(), &text)).await
}

#[tauri::command]
async fn memories_edit(id: i64, text: String) -> Result<atalaya_core::memories::Memory, String> {
    blocking(move |r| atalaya_core::memories::edit(r, id, &text)).await
}

#[tauri::command]
async fn commands_list() -> Result<Vec<atalaya_core::commands::SavedCommand>, String> {
    blocking(atalaya_core::commands::list).await
}

#[tauri::command]
async fn commands_save(name: String, description: String, template: String) -> Result<atalaya_core::commands::SavedCommand, String> {
    blocking(move |r| atalaya_core::commands::save(r, &name, &description, &template)).await
}

#[tauri::command]
async fn commands_examples() -> Result<usize, String> {
    blocking(atalaya_core::commands::add_examples).await
}

#[tauri::command]
async fn commands_remove(name: String) -> Result<(), String> {
    blocking(move |r| atalaya_core::commands::remove(r, &name)).await
}

/* ------------------------------------------------------------------ Servidores MCP externos */

/// Los servidores MCP externos (sin llaves) y los conocidos que se pueden agregar.
#[tauri::command]
async fn mcps_list() -> Result<Value, String> {
    blocking(|r| {
        Ok(serde_json::json!({
            "servers": atalaya_core::mcp_servers::list(r)?,
            "presets": atalaya_core::mcp_servers::PRESETS,
        }))
    })
    .await
}

/// Agrega o cambia un servidor. La llave va directo al Llavero y nunca vuelve a la interfaz.
#[tauri::command]
async fn mcps_save(server: atalaya_core::mcp_servers::Server, secret: Option<String>) -> Result<atalaya_core::mcp_servers::Server, String> {
    blocking(move |r| atalaya_core::mcp_servers::save(r, &server, secret.as_deref())).await
}

/// Arranca el servidor y devuelve sus herramientas.
#[tauri::command]
async fn mcps_test(name: String) -> Result<Vec<String>, String> {
    blocking(move |r| atalaya_core::mcp_servers::test(r, &name)).await
}

#[tauri::command]
async fn mcps_configure(
    name: String,
    enabled: bool,
    projects: Option<Vec<String>>,
    agents: Option<Vec<String>>,
) -> Result<atalaya_core::mcp_servers::Server, String> {
    blocking(move |r| atalaya_core::mcp_servers::configure(r, &name, enabled, projects, agents)).await
}

/// Lo quita de Atalaya; su llave queda en el Llavero.
#[tauri::command]
async fn mcps_remove(name: String) -> Result<(), String> {
    blocking(move |r| atalaya_core::mcp_servers::remove(r, &name)).await
}

/// Las instrucciones para todas las IA (`slug` vacío) o para un proyecto.
#[tauri::command]
async fn instructions_set(slug: Option<String>, text: String) -> Result<(), String> {
    blocking(move |r| atalaya_core::skills::set_instructions(r, slug.as_deref().filter(|s| !s.is_empty()), &text)).await
}

async fn blocking<T: Send + 'static>(
    task: impl FnOnce(&Registry) -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || -> anyhow::Result<T> {
        let registry = Registry::open_default()?;
        task(&registry)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

/// Conexiones guardadas (sin llaves), proveedores, tipos y roles.
#[tauri::command]
async fn connections_list() -> Result<Value, String> {
    blocking(|r| atalaya_core::connections::list(r)).await
}

/// Crea o edita una conexión. La llave va al Llavero y nunca vuelve a la ventana.
#[tauri::command]
async fn connections_save(input: atalaya_core::connections::Input) -> Result<Value, String> {
    blocking(move |r| {
        let saved = atalaya_core::connections::save(r, &input)?;
        Ok(serde_json::json!({ "id": saved.id, "name": saved.name }))
    })
    .await
}

#[tauri::command]
async fn connections_test(id: String) -> Result<String, String> {
    blocking(move |r| atalaya_core::connections::test(r, &id)).await
}

/// Quita una conexión. Solo borra su llave del Llavero si se pide expresamente.
#[tauri::command]
async fn connections_remove(id: String, forget_secret: bool) -> Result<(), String> {
    blocking(move |r| atalaya_core::connections::remove(r, &id, forget_secret)).await
}

/// Vínculos de un proyecto con sus conexiones.
#[tauri::command]
async fn links_get(slug: String) -> Result<Vec<atalaya_core::connections::Link>, String> {
    blocking(move |r| atalaya_core::connections::links(r, &slug)).await
}

#[tauri::command]
async fn links_set(slug: String, role: String, connection_id: String, config: Value) -> Result<(), String> {
    blocking(move |r| atalaya_core::connections::link(r, &slug, &role, &connection_id, &config)).await
}

#[tauri::command]
async fn links_unlink(slug: String, role: String, connection_id: String) -> Result<bool, String> {
    blocking(move |r| atalaya_core::connections::unlink(r, &slug, &role, &connection_id)).await
}

/* ------------------------------------------------------------------- IA */

fn connection(r: &Registry, id: &str) -> anyhow::Result<atalaya_core::connections::Connection> {
    use anyhow::Context;
    atalaya_core::connections::get(r, id)?.with_context(|| format!("no existe la conexión '{id}'"))
}

/// Consumo del mes de cada IA: por conexión y modelo, por tarea, presupuestos y precios.
#[tauri::command]
async fn ai_usage() -> Result<Value, String> {
    blocking(|r| atalaya_core::ai::usage_summary(r)).await
}

#[tauri::command]
async fn ai_models(connection_id: String) -> Result<Vec<String>, String> {
    blocking(move |r| atalaya_core::ai::models(r, &connection(r, &connection_id)?)).await
}

#[tauri::command]
async fn ai_set_price(connection_id: String, model: String, input: f64, output: f64) -> Result<(), String> {
    blocking(move |r| atalaya_core::ai::set_price(r, &connection_id, &model, input, output)).await
}

#[tauri::command]
async fn ai_set_budget(connection_id: String, usd: Option<f64>) -> Result<(), String> {
    blocking(move |r| atalaya_core::ai::set_budget(r, &connection_id, usd)).await
}

/// Quita la pausa por falta de cuota: la IA vuelve a recibir trabajo desde ya.
#[tauri::command]
async fn ai_quota_clear(connection_id: String) -> Result<(), String> {
    blocking(move |r| atalaya_core::quota::clear(r, &connection_id)).await
}

/// Tareas programadas, reacciones e historial.
#[tauri::command]
async fn automations_list() -> Result<Value, String> {
    blocking(atalaya_core::automations::summary).await
}

#[tauri::command]
async fn schedules_save(input: atalaya_core::automations::Schedule) -> Result<atalaya_core::automations::Schedule, String> {
    blocking(move |r| atalaya_core::automations::save_schedule(r, &input)).await
}

#[tauri::command]
async fn schedules_enable(id: i64, on: bool) -> Result<(), String> {
    blocking(move |r| atalaya_core::automations::set_schedule_enabled(r, id, on)).await
}

#[tauri::command]
async fn schedules_remove(id: i64) -> Result<(), String> {
    blocking(move |r| atalaya_core::automations::remove_schedule(r, id)).await
}

/// Corre una tarea ahora, le toque o no.
#[tauri::command]
async fn schedules_run(id: i64) -> Result<atalaya_core::automations::Run, String> {
    blocking(move |r| atalaya_core::automations::run_now(r, id)).await
}

#[tauri::command]
async fn reactions_save(input: atalaya_core::automations::Reaction) -> Result<atalaya_core::automations::Reaction, String> {
    blocking(move |r| atalaya_core::automations::save_reaction(r, &input)).await
}

#[tauri::command]
async fn reactions_enable(id: i64, on: bool) -> Result<(), String> {
    blocking(move |r| atalaya_core::automations::set_reaction_enabled(r, id, on)).await
}

#[tauri::command]
async fn reactions_remove(id: i64) -> Result<(), String> {
    blocking(move |r| atalaya_core::automations::remove_reaction(r, id)).await
}

/// El tope de gasto de cada encargo, en dólares. `None` o 0 lo quita.
#[tauri::command]
async fn ai_set_task_cap(usd: Option<f64>) -> Result<(), String> {
    blocking(move |r| atalaya_core::ai::set_task_cap(r, usd)).await
}

/// Cada respuesta de una comparación, apenas llega.
#[derive(Clone, serde::Serialize)]
struct CompareEvent {
    request: String,
    index: usize,
    answer: atalaya_core::compare::Answer,
}

/// La misma pregunta a varias IA a la vez. Cada respuesta llega como evento «comparar».
#[tauri::command]
async fn chat_compare(
    app: tauri::AppHandle,
    request: String,
    question: String,
    ais: Vec<String>,
    slug: Option<String>,
) -> Result<Vec<atalaya_core::compare::Answer>, String> {
    use tauri::Emitter;
    blocking(move |r| {
        let emit = |index: usize, answer: &atalaya_core::compare::Answer| {
            let _ = app.emit("comparar", CompareEvent { request: request.clone(), index, answer: answer.clone() });
        };
        atalaya_core::compare::ask_many(r, &ais, &question, slug.as_deref(), &emit)
    })
    .await
}

/// Una pregunta corta a un modelo, para probarlo. Queda anotada en el consumo.
#[tauri::command]
async fn ai_ask(connection_id: String, model: Option<String>, prompt: String) -> Result<atalaya_core::ai::Reply, String> {
    blocking(move |r| {
        let c = connection(r, &connection_id)?;
        let req = atalaya_core::ai::Request {
            model,
            messages: vec![atalaya_core::ai::Message::user(prompt)],
            max_tokens: 512,
            task: "prueba".into(),
            ..Default::default()
        };
        atalaya_core::ai::complete(r, &c, &req)
    })
    .await
}

/// Roles de IA (constructor, revisor, validador, asistente): quién está asignado y quién puede elegirse.
#[tauri::command]
async fn ai_roles() -> Result<Value, String> {
    blocking(|r| atalaya_core::orchestrator::summary(r)).await
}

/// Los agentes de terminal que funcionan con tu cuenta: instalados, con sesión, conectados.
#[tauri::command]
async fn ai_clis() -> Result<Vec<atalaya_core::clis::State>, String> {
    blocking(|r| atalaya_core::clis::states(r)).await
}

/// Abre la Terminal para iniciar sesión en un agente.
#[tauri::command]
async fn ai_cli_login(id: String) -> Result<(), String> {
    blocking(move |_| atalaya_core::clis::login(&id)).await
}

/// Abre la Terminal para instalar un agente.
#[tauri::command]
async fn ai_cli_install(id: String) -> Result<(), String> {
    blocking(move |_| atalaya_core::clis::install(&id)).await
}

/// Vuelve a mirar un agente (después de instalarlo o de iniciar sesión).
#[tauri::command]
async fn ai_cli_check(id: String) -> Result<atalaya_core::clis::State, String> {
    blocking(move |r| {
        atalaya_core::clis::forget(&id);
        let cli = atalaya_core::clis::get(&id).ok_or_else(|| anyhow::anyhow!("agente desconocido '{id}'"))?;
        atalaya_core::clis::state(r, cli)
    })
    .await
}

/// Valida un agente de verdad: le hace una pregunta corta dentro de su encierro.
#[tauri::command]
async fn ai_cli_verify(id: String) -> Result<String, String> {
    blocking(move |r| atalaya_core::clis::verify(r, &id)).await
}

/// Deja el agente listo para elegirlo en los roles.
#[tauri::command]
async fn ai_cli_connect(id: String) -> Result<String, String> {
    blocking(move |r| atalaya_core::clis::connect(r, &id)).await
}

/// Los modelos que un agente puede usar en vez de su cuenta (de este Mac o de un servicio).
#[tauri::command]
async fn ai_model_sources() -> Result<Vec<atalaya_core::clis::ModelSource>, String> {
    blocking(atalaya_core::clis::model_sources).await
}

/// Elige el modelo de un agente; sin `source` vuelve a usar su cuenta.
#[tauri::command]
async fn ai_cli_model(id: String, source: Option<atalaya_core::clis::ModelSource>) -> Result<String, String> {
    blocking(move |r| atalaya_core::clis::set_model(r, &id, source.as_ref())).await
}

/// Los servidores de modelos de este Mac (Ollama, LM Studio, Jan, llama.cpp) y sus modelos.
#[tauri::command]
async fn ai_local() -> Result<Vec<atalaya_core::local::Found>, String> {
    blocking(|_| Ok(atalaya_core::local::detect())).await
}

#[tauri::command]
async fn ai_local_add(server: String, model: String) -> Result<String, String> {
    blocking(move |r| atalaya_core::local::add(r, &server, &model)).await
}

#[tauri::command]
async fn ai_local_start(server: String) -> Result<(), String> {
    blocking(move |_| atalaya_core::local::start_server(&server)).await
}

#[tauri::command]
async fn ai_local_open(server: String) -> Result<(), String> {
    blocking(move |_| atalaya_core::local::open_app(&server)).await
}

/// Servicios con la API de OpenAI, para llenar el formulario de una conexión.
#[tauri::command]
fn ai_presets() -> Vec<atalaya_core::connections::Preset> {
    atalaya_core::connections::API_PRESETS.to_vec()
}

#[tauri::command]
async fn ai_set_role(role: String, candidates: Vec<String>, strategy: String) -> Result<Value, String> {
    blocking(move |r| {
        atalaya_core::orchestrator::set_config(r, &role, &atalaya_core::orchestrator::RoleConfig { candidates, strategy })?;
        atalaya_core::orchestrator::summary(r)
    })
    .await
}

/// Le responde al agente de una mejora; la app la vuelve a poner en la cola.
#[tauri::command]
async fn answer_improvement(id: i64, answer: String) -> Result<atalaya_core::Improvement, String> {
    blocking(move |r| r.answer_improvement(id, &answer)).await
}

/// La vista que pidió abrir la última notificación, si llegó hace menos de una hora.
#[tauri::command]
async fn take_notice() -> Result<Option<String>, String> {
    blocking(|r| atalaya_core::checks::take_notice(r)).await
}

/// «Claude Code coordina»: si está encendido y con qué equipo cuenta.
#[tauri::command]
async fn ai_coordination() -> Result<Value, String> {
    blocking(|r| atalaya_core::delegate::summary(r)).await
}

#[tauri::command]
async fn ai_set_coordination(on: bool) -> Result<Value, String> {
    blocking(move |r| {
        atalaya_core::delegate::set_enabled(r, on)?;
        atalaya_core::delegate::summary(r)
    })
    .await
}

/* ---------------------------------------------------- Carpetas de proyectos */

#[tauri::command]
async fn roots_list() -> Result<Vec<String>, String> {
    blocking(|r| Ok(atalaya_core::discover::roots(r)?.iter().map(|p| p.display().to_string()).collect())).await
}

#[tauri::command]
async fn roots_add(path: String) -> Result<String, String> {
    blocking(move |r| Ok(atalaya_core::discover::add_root(r, std::path::Path::new(&path))?.display().to_string())).await
}

/// Quita una carpeta de la lista. No toca la carpeta ni los proyectos registrados.
#[tauri::command]
async fn roots_remove(path: String) -> Result<bool, String> {
    blocking(move |r| atalaya_core::discover::remove_root(r, std::path::Path::new(&path))).await
}

/// Busca proyectos en las carpetas elegidas.
#[tauri::command]
async fn scan_projects() -> Result<Vec<atalaya_core::discover::Found>, String> {
    blocking(|r| atalaya_core::discover::scan(r)).await
}

/// Registra las carpetas elegidas y, si se pide, crea los vínculos propuestos.
#[tauri::command]
async fn register_projects(paths: Vec<String>, link: bool) -> Result<Value, String> {
    blocking(move |r| {
        let (projects, links) = atalaya_core::discover::register(r, &paths, link)?;
        Ok(serde_json::json!({ "projects": projects.len(), "links": links }))
    })
    .await
}

/// Consulta Claude Platform ahora.
#[tauri::command]
async fn claude_refresh() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| -> anyhow::Result<Value> {
        let registry = Registry::open_default()?;
        atalaya_core::claude::refresh(&registry)?;
        atalaya_core::claude::status(&registry)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

#[tauri::command]
async fn claude_set_budget(usd: f64) -> Result<Value, String> {
    with_registry(|r| {
        r.set_setting(atalaya_core::claude::BUDGET, &usd.max(0.0).to_string())?;
        atalaya_core::claude::status(r)
    })
}

#[tauri::command]
async fn system_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(|| -> anyhow::Result<Value> {
        atalaya_core::status::system_status(&Registry::open_default()?)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

/// Ejecuta ahora todas las revisiones de un proyecto. Tardan varios segundos,
/// así que corren fuera del hilo de la interfaz.
#[tauri::command]
async fn run_checks(slug: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let registry = Registry::open_default()?;
        atalaya_core::checks::run_due(&registry, Some(&slug), true)?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(text)
}

/// La puerta de administración habla con la app por la red: fuera del hilo de la interfaz.
async fn admin_task<T: Send + 'static>(
    task: impl FnOnce(&Registry) -> anyhow::Result<T> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || task(&Registry::open_default()?))
        .await
        .map_err(|e| e.to_string())?
        .map_err(text)
}

#[tauri::command]
async fn admin_door(slug: String) -> Result<atalaya_core::admin::Door, String> {
    admin_task(move |r| atalaya_core::admin::door(r, &slug)).await
}

#[tauri::command]
async fn admin_connect(slug: String, url: String) -> Result<atalaya_core::admin::Connected, String> {
    admin_task(move |r| atalaya_core::admin::connect(r, &slug, &url, false)).await
}

#[tauri::command]
async fn admin_schema(slug: String) -> Result<Value, String> {
    admin_task(move |r| atalaya_core::admin::schema(r, &slug)).await
}

#[tauri::command]
async fn admin_apply(slug: String, change: Value) -> Result<String, String> {
    admin_task(move |r| atalaya_core::admin::apply(r, &slug, &change, "app")).await
}

/// Las operaciones de interruptores hablan con Vercel, así que corren fuera del hilo de la interfaz.
async fn flags_task(
    task: impl FnOnce(&Registry) -> anyhow::Result<flags::Flags> + Send + 'static,
) -> Result<flags::Flags, String> {
    tauri::async_runtime::spawn_blocking(move || task(&Registry::open_default()?))
        .await
        .map_err(|e| e.to_string())?
        .map_err(text)
}

#[tauri::command]
async fn list_flags(slug: String) -> Result<flags::Flags, String> {
    flags_task(move |r| flags::list(r, &slug)).await
}

#[tauri::command]
async fn set_flag(slug: String, name: String, on: bool) -> Result<flags::Flags, String> {
    flags_task(move |r| flags::set(r, &slug, &name, on, "app")).await
}

#[tauri::command]
async fn add_flag(slug: String, name: String, desc: String) -> Result<flags::Flags, String> {
    flags_task(move |r| flags::add(r, &slug, &name, &desc, false, "app")).await
}

#[tauri::command]
async fn set_flag_text(slug: String, name: String, text: String) -> Result<flags::Flags, String> {
    flags_task(move |r| flags::set_value(r, &slug, &name, &text, "app")).await
}

#[tauri::command]
async fn remove_flag(slug: String, name: String) -> Result<flags::Flags, String> {
    flags_task(move |r| flags::remove(r, &slug, &name, "app")).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Una sola Atalaya a la vez: dos copias se pisarían la cola del agente.
        // Al abrirla de nuevo, se muestra la ventana de la que ya está corriendo.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| tray::show_window(app)))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Las notificaciones salen a nombre de esta app (su ícono, y se abre al hacer clic),
            // también las de los procesos que lanza, como el servidor MCP del agente.
            if std::env::var_os(atalaya_core::checks::APP_ID_ENV).is_none() {
                // SAFETY: al arrancar todavía no hay otros hilos leyendo el entorno.
                unsafe { std::env::set_var(atalaya_core::checks::APP_ID_ENV, &app.config().identifier) };
            }
            tray::setup(app)?;
            work::recover_interrupted();
            Ok(())
        })
        // Cerrar la ventana la oculta: Atalaya sigue en la barra de menú hasta "Salir".
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            admin_door,
            admin_connect,
            admin_schema,
            admin_apply,
            overview,
            ack_alert,
            resolve_alert,
            set_auto_merge,
            list_improvements,
            add_improvement,
            set_improvement_status,
            delete_improvement,
            system_status,
            doctor,
            list_decisions,
            telegram_decisions,
            conversations_save,
            conversations_list,
            conversations_search,
            conversations_get,
            conversations_recap,
            conversations_export,
            approve_decision,
            discard_decision,
            run_round,
            radar,
            radar_add,
            radar_remove,
            radar_check,
            site_history,
            clone_project,
            backup_project,
            claude_status,
            claude_refresh,
            claude_set_budget,
            connections_list,
            connections_save,
            connections_test,
            connections_remove,
            links_get,
            links_set,
            links_unlink,
            roots_list,
            ai_usage,
            ai_models,
            ai_set_price,
            ai_set_budget,
            ai_quota_clear,
            ai_ask,
            ai_roles,
            ai_set_role,
            ai_coordination,
            ai_set_coordination,
            take_notice,
            answer_improvement,
            ai_clis,
            ai_cli_login,
            ai_cli_install,
            ai_cli_check,
            ai_cli_verify,
            ai_model_sources,
            ai_cli_model,
            ai_cli_connect,
            ai_local,
            ai_local_add,
            ai_local_open,
            ai_local_start,
            ai_presets,
            roots_add,
            roots_remove,
            scan_projects,
            register_projects,
            github_repos,
            github_create,
            github_watch,
            remove_project,
            chat::chat_send,
            chat::chat_stop,
            chat::chat_pending,
            chat::chat_activity,
            chat::chat_attach_data,
            chat::chat_attach_paths,
            skills_list,
            skills_install_folder,
            skills_install_git,
            skills_review,
            skills_configure,
            skills_remove,
            mcps_list,
            memories_list,
            memories_decide,
            memories_add,
            memories_edit,
            commands_list,
            commands_save,
            commands_examples,
            commands_remove,
            ai_set_task_cap,
            chat_compare,
            automations_list,
            schedules_save,
            schedules_enable,
            schedules_remove,
            schedules_run,
            reactions_save,
            reactions_enable,
            reactions_remove,
            mcps_save,
            mcps_test,
            mcps_configure,
            mcps_remove,
            instructions_set,
            chat::chat_answer,
            work::start_improvement,
            work::queued_improvements,
            work::merge_improvement,
            project_context,
            run_checks,
            list_flags,
            set_flag,
            add_flag,
            set_flag_text,
            remove_flag
        ])
        .build(tauri::generate_context!())
        .expect("no se pudo iniciar Atalaya")
        .run(|app, event| {
            // Clic en el ícono del Dock con la ventana oculta: se vuelve a mostrar.
            // Al salir, se detienen los agentes en marcha.
            if let tauri::RunEvent::Exit = event {
                atalaya_core::worker::stop_all();
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                tray::show_window(app);
                // La vista lee si la reabrió una notificación y va a lo que avisaba.
                let _ = tauri::Emitter::emit(app, "reabrir", ());
            }
            let _ = app;
        });
}
