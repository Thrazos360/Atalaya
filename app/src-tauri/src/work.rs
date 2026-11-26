//! Cola de trabajo del agente: las mejoras aceptadas se trabajan de a una, en segundo
//! plano, y la ventana recibe el avance por eventos `work`.

use atalaya_core::{Improvement, Registry, worker};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
struct WorkEvent {
    id: i64,
    /// `estado`, `paso`, `nota`, `fin` o `error`.
    kind: String,
    text: String,
}

struct Queue {
    waiting: VecDeque<i64>,
    /// Hay un hilo atendiendo la cola.
    busy: bool,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue { waiting: VecDeque::new(), busy: false });

fn emit(app: &AppHandle, id: i64, kind: &str, text: &str) {
    let _ = app.emit("work", WorkEvent { id, kind: kind.into(), text: text.into() });
}

fn drain(app: AppHandle) {
    loop {
        let next = {
            let mut queue = QUEUE.lock().unwrap();
            match queue.waiting.pop_front() {
                Some(id) => id,
                None => {
                    queue.busy = false;
                    return;
                }
            }
        };
        let outcome = Registry::open_default()
            .and_then(|registry| worker::run(&registry, next, &|kind, text| emit(&app, next, kind, text)));
        match outcome {
            Ok(_) => emit(&app, next, "fin", ""),
            Err(e) => emit(&app, next, "error", &format!("{e:#}")),
        }
    }
}

/// Pone una mejora aceptada en la cola del agente.
#[tauri::command]
pub async fn start_improvement(app: AppHandle, id: i64) -> Result<(), String> {
    let mut queue = QUEUE.lock().unwrap();
    if !queue.waiting.contains(&id) {
        queue.waiting.push_back(id);
    }
    if !queue.busy {
        queue.busy = true;
        std::thread::spawn(move || drain(app));
    }
    Ok(())
}

/// Mejoras que esperan su turno en la cola.
#[tauri::command]
pub async fn queued_improvements() -> Vec<i64> {
    QUEUE.lock().unwrap().waiting.iter().copied().collect()
}

/// Fusiona el pull request de una mejora: el cambio pasa a la rama principal.
#[tauri::command]
pub async fn merge_improvement(id: i64) -> Result<Improvement, String> {
    tauri::async_runtime::spawn_blocking(move || worker::merge(&Registry::open_default()?, id))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Al abrir la app no hay ningún agente corriendo: lo que quedó "en curso" de una
/// sesión anterior se interrumpió y vuelve a "aceptada" para poder reintentarlo.
pub fn recover_interrupted() {
    let Ok(registry) = Registry::open_default() else { return };
    for improvement in registry.improvements(None).unwrap_or_default() {
        if improvement.status == "en_curso" {
            let _ = registry.update_improvement_run(
                improvement.id,
                "aceptada",
                None,
                None,
                Some("El trabajo se interrumpió al cerrar Atalaya. Se puede reintentar."),
            );
        }
    }
}
