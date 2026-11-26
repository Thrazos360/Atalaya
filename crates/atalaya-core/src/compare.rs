//! Preguntar a varias IA a la vez: la misma pregunta a dos, tres o cuatro IA, cada una en su
//! hilo, para comparar las respuestas lado a lado. Lo pide la persona desde el chat; no
//! depende de que «Claude Code coordina» esté encendido. Solo leen: los agentes de terminal
//! van en solo lectura, y todo queda anotado en el consumo como «comparar».

use crate::Registry;
use crate::orchestrator::{self, Candidate};
use anyhow::{Result, bail};
use serde::Serialize;
use std::time::Instant;

/// Cuántas IA a la vez.
pub const MAX: usize = 4;

const TASK: &str = "comparar";

const SYSTEM: &str = "El usuario le hizo la misma pregunta a varias IA para comparar las respuestas. Responde en \
español, concreto y verificable: qué recomiendas y por qué. No inventes: si no pudiste comprobar algo, dilo. No \
cambies archivos.";

#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    /// La conexión (o `claude-code`).
    pub id: String,
    pub name: String,
    pub ok: bool,
    /// La respuesta, o por qué no respondió.
    pub text: String,
    pub seconds: f64,
    /// `None` cuando no se conoce el precio (o lo cubre la cuenta de la persona).
    pub cost_usd: Option<f64>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
}

fn last_usage_id(registry: &Registry) -> Result<i64> {
    Ok(registry.conn().query_row("SELECT coalesce(max(id), 0) FROM ai_usage", [], |r| r.get(0))?)
}

/// Lo que gastó esta IA en esta pregunta: lo anotado desde que empezó.
fn spent(registry: &Registry, since: i64, connection_id: &str, task: &str) -> Result<(Option<f64>, Option<i64>, Option<i64>)> {
    Ok(registry.conn().query_row(
        "SELECT sum(cost_usd), sum(input_tokens), sum(output_tokens) FROM ai_usage
         WHERE id > ?1 AND connection_id = ?2 AND task = ?3",
        rusqlite::params![since, connection_id, task],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}

/// Le hace la pregunta a una IA. Nunca falla: si no pudo, la respuesta lo dice.
pub fn ask_one(registry: &Registry, id: &str, question: &str, slug: Option<&str>) -> Answer {
    ask_as(registry, id, SYSTEM, question, slug, TASK)
}

/// Como `ask_one`, con otra instrucción y otra tarea en el consumo (las tareas programadas y
/// las reacciones lo usan).
pub fn ask_as(registry: &Registry, id: &str, system: &str, question: &str, slug: Option<&str>, task: &str) -> Answer {
    let started = Instant::now();
    let mut answer = Answer {
        id: id.to_string(),
        name: id.to_string(),
        ok: false,
        text: String::new(),
        seconds: 0.0,
        cost_usd: None,
        input_tokens: None,
        output_tokens: None,
    };
    let result = (|| -> Result<String> {
        let c = orchestrator::resolve(registry, id)?.ok_or_else(|| anyhow::anyhow!("la IA «{id}» ya no existe"))?;
        answer.name = c.label();
        if let Some(why) = orchestrator::unavailable(registry, &c)? {
            bail!("no está disponible: {why}");
        }
        let folder = crate::delegate::folder_of_project(registry, slug)?;
        let prompt = crate::delegate::with_project(registry, &c, question, slug, folder.as_deref())?;
        let since = last_usage_id(registry)?;
        let usage_id = match &c {
            Candidate::ClaudeCode => crate::ai::CLAUDE_CODE_ID.to_string(),
            Candidate::Connection(conn) => conn.id.clone(),
        };
        let text = orchestrator::ask(registry, &c, system, &prompt, task, slug);
        (answer.cost_usd, answer.input_tokens, answer.output_tokens) = spent(registry, since, &usage_id, task)?;
        text
    })();
    answer.seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    match result {
        Ok(text) => {
            answer.ok = true;
            answer.text = text.trim().to_string();
        }
        Err(e) => answer.text = format!("{e:#}"),
    }
    answer
}

/// La misma pregunta a varias IA a la vez, cada una con su conexión a la base. `on_answer`
/// recibe cada respuesta apenas llega (con su posición), para mostrarla sin esperar a las
/// demás. Devuelve todas en el orden pedido.
pub fn ask_many(
    registry: &Registry,
    ids: &[String],
    question: &str,
    slug: Option<&str>,
    on_answer: &(dyn Fn(usize, &Answer) + Sync),
) -> Result<Vec<Answer>> {
    if question.trim().is_empty() {
        bail!("escribe la pregunta");
    }
    let mut unique: Vec<&String> = Vec::new();
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.len() < 2 {
        bail!("elige al menos dos IA para comparar");
    }
    if unique.len() > MAX {
        bail!("son {} IA; compara como máximo {MAX} a la vez", unique.len());
    }
    let path = registry.path();
    Ok(std::thread::scope(|scope| {
        let handles: Vec<_> = unique
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let path = &path;
                scope.spawn(move || {
                    let answer = match Registry::open(path) {
                        Ok(r) => ask_one(&r, id, question, slug),
                        Err(e) => Answer {
                            id: id.to_string(),
                            name: id.to_string(),
                            ok: false,
                            text: format!("{e:#}"),
                            seconds: 0.0,
                            cost_usd: None,
                            input_tokens: None,
                            output_tokens: None,
                        },
                    };
                    on_answer(i, &answer);
                    answer
                })
            })
            .collect();
        handles
            .into_iter()
            .zip(&unique)
            .map(|(h, id)| {
                h.join().unwrap_or_else(|_| Answer {
                    id: id.to_string(),
                    name: id.to_string(),
                    ok: false,
                    text: "se cayó mientras respondía".into(),
                    seconds: 0.0,
                    cost_usd: None,
                    input_tokens: None,
                    output_tokens: None,
                })
            })
            .collect()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{Input, save};
    use serde_json::json;

    fn agent(r: &Registry, dir: &std::path::Path, name: &str) -> String {
        let script = dir.join(format!("{name}.sh"));
        std::fs::write(&script, format!("#!/bin/sh\nsleep 2\necho \"{name} opina\"\n")).unwrap();
        std::process::Command::new("chmod").arg("+x").arg(&script).status().unwrap();
        let command = format!("{} {{tarea}}", script.display());
        save(
            r,
            &Input {
                id: None,
                provider: "agente-personalizado".into(),
                name: name.into(),
                config: json!({ "command": command, "ask_command": command }),
                secret: None,
            },
        )
        .unwrap()
        .id
    }

    #[test]
    fn same_question_to_several_at_once() {
        let dir = std::env::temp_dir().join(format!("atalaya-comparar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = Registry::open(&dir.join("a.db")).unwrap();
        let uno = agent(&r, &dir, "Uno");
        let dos = agent(&r, &dir, "Dos");

        assert!(ask_many(&r, &[uno.clone()], "¿x?", None, &|_, _| {}).unwrap_err().to_string().contains("al menos dos"));
        assert!(ask_many(&r, &[uno.clone(), dos.clone()], "  ", None, &|_, _| {}).is_err());

        let seen = std::sync::Mutex::new(Vec::new());
        let start = Instant::now();
        let ids = [uno.clone(), dos.clone(), "no-existe".to_string()];
        let answers = ask_many(&r, &ids, "¿Qué opinas?", None, &|i, a| seen.lock().unwrap().push((i, a.ok))).unwrap();
        // Cada una tarda 2 s: una tras otra no bajarían de 4 s.
        assert!(start.elapsed().as_secs_f64() < 3.9, "responden a la vez, no una tras otra");
        assert_eq!(answers.len(), 3);
        assert!(answers[0].ok && answers[0].text.contains("Uno opina"), "{:?}", answers[0]);
        assert_eq!(answers[1].name, "Dos");
        assert!(!answers[2].ok && answers[2].text.contains("ya no existe"), "{:?}", answers[2]);
        assert_eq!(seen.lock().unwrap().len(), 3, "cada respuesta se avisa apenas llega");

        let usage = crate::ai::usage_summary(&r).unwrap();
        assert_eq!(usage["by_task"][0]["task"], "comparar");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cap_limits_what_an_api_model_can_spend() {
        let dir = std::env::temp_dir().join(format!("atalaya-tope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = Registry::open(&dir.join("a.db")).unwrap();
        assert_eq!(crate::ai::claude_cap_args(&r).unwrap(), Vec::<String>::new());
        crate::ai::set_task_cap(&r, Some(0.5)).unwrap();
        assert_eq!(crate::ai::claude_cap_args(&r).unwrap(), vec!["--max-budget-usd".to_string(), "0.5".into()]);
        assert!(crate::ai::set_task_cap(&r, Some(-1.0)).is_err());
        let note = crate::ai::claude_cap_note(&json!({"subtype": "error_max_budget_usd", "total_cost_usd": 0.61})).unwrap();
        assert!(note.contains("US$ 0.61"), "{note}");
        assert_eq!(crate::ai::claude_cap_note(&json!({"subtype": "success"})), None);
        crate::ai::set_task_cap(&r, None).unwrap();
        assert_eq!(crate::ai::task_cap(&r).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
