//! Un agente de terminal encerrado consulta Atalaya por su MCP de solo lectura, a través del
//! puente (`atalaya mcp-puente`), sin poder leer la base ni tocar la carpeta del puente.

use atalaya_core::{Project, Registry, agents};
use std::time::Duration;

const MENSAJES: &[&str] = &[
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"agente-falso","version":"1"}}}"#,
    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
    r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_projects","arguments":{}}}"#,
    r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"set_flag","arguments":{"slug":"faro","name":"x","on":true}}}"#,
];

#[test]
fn a_sandboxed_agent_reads_atalaya_through_its_mcp() {
    let root = std::env::temp_dir().join(format!("atalaya-mcp-e2e-{}", std::process::id()));
    let data = root.join("datos");
    let work = root.join("trabajo");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let db = data.join("atalaya.db");
    // Única prueba de este binario: nadie más lee el entorno mientras cambia.
    unsafe {
        std::env::set_var("ATALAYA_DB", &db);
        std::env::set_var("ATALAYA_KEYCHAIN_SERVICE", "Atalaya-prueba-mcp");
        std::env::set_var("ATALAYA_CLI", env!("CARGO_BIN_EXE_atalaya"));
    }
    let registry = Registry::open(&db).unwrap();
    registry
        .upsert(&Project {
            slug: "faro".into(),
            name: "Faro del Sur".into(),
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

    let messages: String = MENSAJES.iter().map(|m| format!("'{m}' ")).collect();
    let script = root.join("agente.sh");
    std::fs::write(
        &script,
        format!(
            r#"cfg="$ATALAYA_MCP_CONFIG"
cmd=$(sed -E 's/.*"command":"([^"]+)".*/\1/' "$cfg")
file=$(sed -E 's/.*"mcp-puente","([^"]+)".*/\1/' "$cfg")
if cat "$BASE_PRUEBA" >/dev/null 2>&1; then echo "lee-base: si"; else echo "lee-base: no"; fi
if (echo x >> "$file") 2>/dev/null; then echo "escribe-puente: si"; else echo "escribe-puente: no"; fi
{{ printf '%s\n' {messages}; sleep 2; }} | "$cmd" mcp-puente "$file"
"#
        ),
    )
    .unwrap();

    let mut policy = agents::Policy::for_workdir(&work);
    policy.env.push(("BASE_PRUEBA".into(), db.display().to_string()));
    let run =
        agents::run_template(&format!("/bin/sh {} {{tarea}}", script.display()), &work, "x", &policy, Duration::from_secs(60)).unwrap();
    let out = run.output;

    assert!(out.contains("lee-base: no"), "{out}");
    assert!(out.contains("escribe-puente: no"), "{out}");
    let line = |id: u32| out.lines().find(|l| l.contains(&format!("\"id\":{id}"))).unwrap_or_else(|| panic!("sin respuesta {id}:\n{out}"));
    let tools = line(2);
    assert!(tools.contains("list_projects") && tools.contains("readOnlyHint"), "{tools}");
    assert!(!tools.contains("set_flag") && !tools.contains("delegar_trabajo"), "{tools}");
    assert!(line(3).contains("Faro del Sur"), "{}", line(3));
    let write = line(4);
    assert!(write.contains("error") || write.contains("isError\":true"), "{write}");

    let _ = std::fs::remove_dir_all(&root);
}
