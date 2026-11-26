//! Ayudas para las pruebas: base temporal, proyecto de ejemplo y un servidor de IA falso.

use crate::{Project, Registry};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn registry(name: &str) -> Registry {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!("atalaya-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Registry::open(&dir.join("atalaya.db")).unwrap()
}

pub fn project(registry: &Registry, slug: &str) {
    registry
        .upsert(&Project {
            slug: slug.into(),
            name: slug.into(),
            description: None,
            local_path: None,
            repo: None,
            production_url: None,
            notes: None,
            vercel_project_id: None,
            vercel_team_id: None,
            supabase_ref: None,
            updated_at: String::new(),
        })
        .unwrap();
}

/// Un servidor compatible con OpenAI que responde siempre `content`. Devuelve su URL base.
pub fn chat_server(content: String) -> String {
    let reply = serde_json::json!({
        "choices": [{ "message": { "content": content } }],
        "usage": { "prompt_tokens": 100, "completion_tokens": 50 }
    })
    .to_string();
    json_server(reply)
}

/// Un servidor que responde siempre este JSON (por ejemplo, una lista de modelos).
pub fn models_server(json: &str) -> String {
    json_server(json.to_string())
}

fn json_server(reply: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
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
            let mut stream = stream;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    format!("http://{addr}/v1")
}

/// Una conexión de IA local apuntando a `url`.
pub fn local_ai(registry: &Registry, name: &str, url: &str, model: &str) -> crate::connections::Connection {
    crate::connections::save(
        registry,
        &crate::connections::Input {
            id: None,
            provider: "ollama".into(),
            name: name.into(),
            config: serde_json::json!({ "base_url": url, "model": model }),
            secret: None,
        },
    )
    .unwrap()
}
