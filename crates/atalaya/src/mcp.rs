//! Servidor MCP de solo lectura: le da a la sesión de IA el contexto de cada proyecto.

use anyhow::Result;
use atalaya_core::Registry;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize, schemars::JsonSchema)]
struct PermissionArgs {
    /// Herramienta que Claude Code quiere usar (Bash, Edit, Write…).
    tool_name: String,
    /// Lo que le pasaría a esa herramienta.
    #[serde(default)]
    input: Value,
    /// Identificador de esa llamada.
    #[serde(default)]
    tool_use_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SlugArgs {
    /// Identificador del proyecto, tal como lo devuelve list_projects.
    slug: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct SetFlagArgs {
    /// Identificador del proyecto, tal como lo devuelve list_projects.
    slug: String,
    /// Nombre del interruptor, tal como lo devuelve list_flags.
    name: String,
    /// true para prender, false para apagar.
    on: bool,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct AdminSetArgs {
    /// Identificador del proyecto, tal como lo devuelve list_projects.
    slug: String,
    /// Id de la sección, tal como lo devuelve admin_controls (por ejemplo "clientes").
    section: String,
    /// Id del elemento dentro de la sección (por ejemplo el id de un cliente).
    item: String,
    /// Clave del control, tal como la devuelve admin_controls (por ejemplo "plan" o "feature:facturas").
    key: String,
    /// Valor nuevo: true/false para un interruptor, el value de una opción, un número, un texto o null para vaciar.
    value: Value,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct AddImprovementArgs {
    /// Identificador del proyecto, tal como lo devuelve list_projects.
    slug: String,
    /// Título corto de la mejora.
    title: String,
    /// Detalle: qué problema resuelve, evidencia y cómo se haría.
    detail: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ImprovementStatusArgs {
    /// Número de la mejora, tal como lo devuelve list_improvements.
    id: i64,
    /// Nueva etapa: propuesta, aceptada, en_curso, en_revision, hecha o descartada.
    status: String,
    /// Qué hiciste o qué falta, en una o dos frases. Se ve en la tarjeta de la mejora.
    note: Option<String>,
    /// La rama donde quedó el cambio, si hay una.
    branch: Option<String>,
    /// El pull request del cambio, si lo abriste.
    pr_url: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ProposeArgs {
    /// Identificador del proyecto, tal como lo devuelve list_projects.
    slug: String,
    /// De dónde sale: alerta, issue, radar, mantenimiento.
    kind: String,
    /// Título corto y concreto.
    title: String,
    /// La evidencia: qué se observó y dónde.
    context: String,
    /// Lo que se propone hacer o decir, listo para aprobar tal cual.
    proposal: String,
    /// Qué pasa al aprobar: mejora, comentario, reconocer o ninguna.
    action: String,
    /// Número del issue (acción comentario) o tipo de revisión (acción reconocer).
    target: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct StatusArgs {
    /// Identificador del proyecto. Si se omite, devuelve el estado de todos.
    slug: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct CommitsArgs {
    /// Identificador del proyecto, tal como lo devuelve list_projects.
    slug: String,
    /// Cuántos commits devolver (por defecto 20, máximo 200).
    limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ConsultArgs {
    /// Lo que le pides, completo: la otra IA no ve esta conversación.
    tarea: String,
    /// Id de la IA del equipo (de tu instrucción de sistema). Si se omite, la elige el rol.
    ia: Option<String>,
    /// Rol que elige cuando no hay `ia`: validador (por defecto), revisor, constructor o asistente.
    rol: Option<String>,
    /// Slug del proyecto, para que lea su carpeta o reciba su estado.
    proyecto: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct WorkArgs {
    /// El encargo, completo y acotado: qué cambiar, dónde y cómo comprobarlo. La otra IA no ve esta conversación.
    tarea: String,
    /// Slug del proyecto: el agente trabaja en su carpeta.
    proyecto: String,
    /// Id de un agente del equipo que pueda cambiar archivos. Si se omite, lo hace el constructor de los roles.
    ia: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ConsultOrder {
    /// Lo que le pides, completo: la otra IA no ve esta conversación ni los demás encargos.
    tarea: String,
    /// Id de la IA del equipo. Si se omite, el reparto elige a otro del rol para cada encargo.
    ia: Option<String>,
    /// Rol del que se reparte cuando no hay `ia`: validador (por defecto), revisor, constructor o asistente.
    rol: Option<String>,
    /// Slug del proyecto, para que lea su carpeta o reciba su estado.
    proyecto: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ConsultsArgs {
    /// Las consultas, cada una independiente de las demás (hasta 6).
    encargos: Vec<ConsultOrder>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct WorkOrder {
    /// El cambio, completo y acotado: qué cambiar, dónde y cómo comprobarlo.
    tarea: String,
    /// Slug del proyecto. Un solo cambio por proyecto en cada reparto.
    proyecto: String,
    /// Id de un agente del equipo que pueda cambiar archivos. Si se omite, se reparte entre los constructores.
    ia: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct WorksArgs {
    /// Los cambios, cada uno en un proyecto distinto (hasta 6).
    encargos: Vec<WorkOrder>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct MemoryArgs {
    /// Lo que conviene recordar, en una o dos frases, tal como lo leería otra IA.
    texto: String,
    /// Slug del proyecto al que se refiere. Si vale para todos, se omite.
    proyecto: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct MemoriesArgs {
    /// Slug del proyecto. Si se omite, todas.
    proyecto: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ResolveArgs {
    /// Número de la decisión (de list_decisions).
    id: i64,
    /// `aprobar` o `descartar`.
    accion: String,
    /// Al aprobar, el texto final si el usuario ajustó la propuesta. Si se omite, se usa la propuesta tal cual.
    texto: Option<String>,
}

/// Las herramientas que solo leen: las únicas del modo `--read-only`, el que reciben los
/// agentes de terminal. Una herramienta nueva queda fuera hasta que se agregue aquí.
const READ_ONLY: &[&str] = &[
    "list_projects",
    "get_project_context",
    "recent_commits",
    "project_status",
    "list_flags",
    "list_improvements",
    "list_decisions",
    "claude_usage",
    "ai_usage",
    "ai_roles",
    "github_issues",
    "radar_events",
    "list_memories",
];

struct Atalaya {
    router: ToolRouter<Atalaya>,
}

impl Atalaya {
    fn new(read_only: bool) -> Self {
        let mut router = Self::tool_router();
        if read_only {
            router.map.retain(|name, _| READ_ONLY.contains(&name.as_ref()));
            for route in router.map.values_mut() {
                let notes = route.attr.annotations.clone().unwrap_or_default();
                route.attr.annotations = Some(notes.read_only(true));
            }
        }
        Atalaya { router }
    }
}

/// La conversación de la vista Agente que lanzó esta sesión de Claude Code, si la hay: ahí
/// se ve en vivo lo que hacen las IA a las que se les encarga algo.
fn chat_conversation() -> Option<String> {
    std::env::var("ATALAYA_CHAT_CONVERSATION").ok().filter(|c| !c.is_empty())
}

/// Como `answer`, para trabajos largos: corre aparte para no frenar el servidor.
async fn answer_slow(f: impl FnOnce(&Registry) -> Result<String> + Send + 'static) -> Result<String, String> {
    tokio::task::spawn_blocking(move || Registry::open_default().and_then(|r| f(&r)))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
}

/// Abre el registro en cada llamada: así siempre ve lo último que escribió la app o la CLI.
fn answer(f: impl FnOnce(&Registry) -> Result<Value>) -> Result<String, String> {
    Registry::open_default()
        .and_then(|r| f(&r))
        .and_then(|v| render(v))
        .map_err(|e| format!("{e:#}"))
}

/// Un texto va tal cual, sin comillas ni `\n` escapados; lo demás, como JSON legible.
fn render(v: Value) -> Result<String> {
    Ok(match v {
        Value::String(text) => text,
        other => serde_json::to_string_pretty(&other)?,
    })
}

#[tool_router]
impl Atalaya {
    #[tool(
        description = "Lista todos los proyectos registrados en Atalaya: slug, nombre, repo de GitHub, carpeta local y URL de producción."
    )]
    async fn list_projects(&self) -> Result<String, String> {
        answer(|r| Ok(serde_json::to_value(r.list()?)?))
    }

    #[tool(
        description = "Contexto completo de un proyecto: datos del registro, rama y últimos commits, dependencias y scripts, servicios conectados (Vercel, Supabase, Firebase) y los documentos CLAUDE.md, AGENTS.md y README.md. Llámalo antes de trabajar en un proyecto."
    )]
    async fn get_project_context(
        &self,
        Parameters(a): Parameters<SlugArgs>,
    ) -> Result<String, String> {
        answer(|r| atalaya_core::project_context(r, &a.slug))
    }

    #[tool(
        description = "Historial reciente de un proyecto: rama actual, archivos sin confirmar y los últimos commits."
    )]
    async fn recent_commits(
        &self,
        Parameters(a): Parameters<CommitsArgs>,
    ) -> Result<String, String> {
        answer(|r| atalaya_core::recent_commits(r, &a.slug, a.limit.unwrap_or(20).min(200)))
    }

    #[tool(
        description = "Estado de vigilancia: resultado de las últimas revisiones (sitio en línea, git, dependencias y vulnerabilidades) de un proyecto o de todos. Cada revisión trae estado ok, aviso o falla."
    )]
    async fn project_status(
        &self,
        Parameters(a): Parameters<StatusArgs>,
    ) -> Result<String, String> {
        answer(|r| {
            let mut all = Vec::new();
            for p in r.list()? {
                if a.slug.as_ref().is_none_or(|s| *s == p.slug) {
                    all.extend(r.latest_checks(&p.slug)?);
                }
            }
            Ok(serde_json::to_value(all)?)
        })
    }

    #[tool(
        description = "Interruptores de funciones de un proyecto: nombre, si está prendido y qué hace cada uno. Las apps los leen en vivo desde Vercel Global Config."
    )]
    async fn list_flags(&self, Parameters(a): Parameters<SlugArgs>) -> Result<String, String> {
        answer(|r| Ok(serde_json::to_value(atalaya_core::flags::list(r, &a.slug)?)?))
    }

    #[tool(
        description = "Prende o apaga un interruptor existente de un proyecto. El cambio llega a producción de inmediato, sin desplegar, así que úsalo solo cuando el usuario lo pida expresamente. Devuelve el estado de todos los interruptores del proyecto."
    )]
    async fn set_flag(&self, Parameters(a): Parameters<SetFlagArgs>) -> Result<String, String> {
        answer(|r| {
            Ok(serde_json::to_value(atalaya_core::flags::set(
                r, &a.slug, &a.name, a.on, "mcp",
            )?)?)
        })
    }

    #[tool(
        description = "Mejoras anotadas de un proyecto (o de todos si se omite el slug): título, detalle, etapa (propuesta, aceptada, en_curso, en_revision, hecha, descartada) y quién la creó. Revísalas antes de trabajar en un proyecto: si lo que vas a hacer ya es una mejora anotada, trabájala y muévela de etapa con set_improvement_status en vez de anotar otra."
    )]
    async fn list_improvements(
        &self,
        Parameters(a): Parameters<StatusArgs>,
    ) -> Result<String, String> {
        answer(|r| Ok(serde_json::to_value(r.improvements(a.slug.as_deref())?)?))
    }

    #[tool(
        description = "Anota una mejora propuesta para un proyecto. Queda como 'propuesta' hasta que el usuario la acepte en Atalaya."
    )]
    async fn add_improvement(
        &self,
        Parameters(a): Parameters<AddImprovementArgs>,
    ) -> Result<String, String> {
        answer(|r| {
            let id = r.add_improvement(&a.slug, &a.title, a.detail.as_deref().unwrap_or(""), "mcp")?;
            Ok(serde_json::json!({ "id": id }))
        })
    }

    #[tool(
        description = "Mueve una mejora a su etapa en el tablero de Mejoras. Cuando trabajes una mejora, llévala por sus etapas: 'en_curso' al empezar, 'en_revision' cuando el cambio esté listo y espere el visto bueno del usuario (con la rama o el PR), 'hecha' cuando quede aplicado y verificado, y de vuelta a 'aceptada' si queda a medias. Agrega una nota con qué hiciste o qué falta. 'descartada' es decisión del usuario."
    )]
    async fn set_improvement_status(
        &self,
        Parameters(a): Parameters<ImprovementStatusArgs>,
    ) -> Result<String, String> {
        answer(|r| {
            let clean = |v: &Option<String>| v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
            let (note, branch, pr_url) = (clean(&a.note), clean(&a.branch), clean(&a.pr_url));
            if note.is_some() || branch.is_some() || pr_url.is_some() {
                r.update_improvement_run(a.id, &a.status, branch.as_deref(), pr_url.as_deref(), note.as_deref())?;
            } else {
                r.set_improvement_status(a.id, &a.status)?;
            }
            Ok(serde_json::json!({ "id": a.id, "status": a.status }))
        })
    }

    #[tool(
        description = "Gasto y uso de la API de Claude en el mes en curso, según Claude Platform: total en dólares, solicitudes, tokens, desglose por modelo, gasto diario y presupuesto mensual. Es el último dato que leyó Atalaya, que se refresca cada hora."
    )]
    async fn claude_usage(&self) -> Result<String, String> {
        answer(atalaya_core::claude::status)
    }

    #[tool(
        description = "Uso interno de la vista Agente de Atalaya: pide al usuario permiso para una acción y espera su respuesta en la app. No la llames directamente."
    )]
    async fn aprobar_accion(&self, Parameters(a): Parameters<PermissionArgs>) -> Result<String, String> {
        // La conversación la fija la app al lanzar a Claude Code; sin ella no hay a quién preguntar.
        let Some(conversation) = std::env::var("ATALAYA_CHAT_CONVERSATION").ok().filter(|c| !c.is_empty()) else {
            return Ok(atalaya_core::approvals::decision_json(false, Some("No hay una conversación de Atalaya que pueda aprobarlo."), &a.input));
        };
        tokio::task::spawn_blocking(move || {
            let registry = Registry::open_default()?;
            atalaya_core::approvals::request_step(
                &registry,
                &conversation,
                &a.tool_name,
                &a.input,
                a.tool_use_id.as_deref(),
                std::time::Duration::from_secs(15 * 60),
            )
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{e:#}"))
    }

    #[tool(
        description = "Consumo del mes de cada IA conectada a Atalaya, según su propio registro: por conexión y modelo (llamadas, tokens, costo o 'precio sin definir'), por tarea, presupuestos y precios conocidos. Solo lectura."
    )]
    async fn ai_usage(&self) -> Result<String, String> {
        answer(atalaya_core::ai::usage_summary)
    }

    #[tool(
        description = "Roles de IA de Atalaya: quién construye las mejoras, quién las revisa, quién hace la ronda diaria y quién responde el chat, con su estrategia y si cada candidato está disponible. Solo lectura."
    )]
    async fn ai_roles(&self) -> Result<String, String> {
        answer(atalaya_core::orchestrator::summary)
    }

    #[tool(
        description = "Decisiones pendientes: propuestas que esperan el visto bueno del usuario. Revísalas antes de proponer algo, para no repetir."
    )]
    async fn list_decisions(&self) -> Result<String, String> {
        answer(|r| Ok(serde_json::to_value(atalaya_core::decisions::list(r, true)?)?))
    }

    #[tool(
        description = "Aprueba o descarta una decisión de la cola de Decisiones. Úsala solo cuando el usuario te lo pide en el chat: las decisiones son suyas. Al aprobar se ejecuta su acción: crear la mejora (queda aceptada), publicar el comentario en el issue o reconocer la alerta."
    )]
    async fn resolve_decision(&self, Parameters(a): Parameters<ResolveArgs>) -> Result<String, String> {
        answer(|r| {
            let text = match a.accion.as_str() {
                "aprobar" => match atalaya_core::decisions::approve(r, a.id, a.texto.as_deref().unwrap_or_default(), false)? {
                    Some(improvement) => format!(
                        "Aprobada. Se creó la mejora #{improvement}, aceptada: trabájala tú aquí o el usuario la manda al agente de Mejoras desde el chat."
                    ),
                    None => "Aprobada y hecha su acción.".to_string(),
                },
                "descartar" => {
                    atalaya_core::decisions::discard(r, a.id)?;
                    "Descartada.".to_string()
                }
                other => anyhow::bail!("acción «{other}» desconocida: usa aprobar o descartar"),
            };
            Ok(Value::String(text))
        })
    }

    #[tool(
        description = "Propone algo para que todas las IA lo recuerden en adelante: una convención del proyecto, una decisión tomada o una preferencia del usuario. Queda esperando que el usuario la apruebe; no propongas lo obvio, lo pasajero ni lo que ya está en las instrucciones."
    )]
    async fn proponer_memoria(&self, Parameters(a): Parameters<MemoryArgs>) -> Result<String, String> {
        answer(|r| {
            let m = atalaya_core::memories::propose(r, a.proyecto.as_deref(), &a.texto, "claude-code")?;
            Ok(serde_json::json!({ "id": m.id, "estado": m.status, "nota": "Queda para que el usuario la apruebe." }))
        })
    }

    #[tool(description = "La memoria aprobada y la propuesta: lo que las IA recuerdan de cada proyecto y del usuario.")]
    async fn list_memories(&self, Parameters(a): Parameters<MemoriesArgs>) -> Result<String, String> {
        answer(|r| {
            let all = atalaya_core::memories::list(r)?;
            let mine: Vec<_> = all.into_iter().filter(|m| a.proyecto.is_none() || m.slug.is_none() || m.slug == a.proyecto).collect();
            Ok(serde_json::to_value(mine)?)
        })
    }

    #[tool(
        description = "Deja una propuesta en la cola de Decisiones para que el usuario la apruebe, edite o descarte. No ejecuta nada: la acción ocurre solo al aprobar. Acciones: `mejora` (crea una mejora que el agente trabaja; `proposal` es el encargo completo), `comentario` (publica `proposal` en el issue de GitHub `target`), `reconocer` (reconoce la alerta del tipo `target`) o `ninguna`."
    )]
    async fn propose_decision(&self, Parameters(a): Parameters<ProposeArgs>) -> Result<String, String> {
        answer(|r| {
            let id = atalaya_core::decisions::propose(
                r, &a.slug, &a.kind, &a.title, &a.context, &a.proposal, &a.action, a.target.as_deref(),
            )?;
            Ok(serde_json::json!({ "id": id }))
        })
    }

    #[tool(description = "Issues abiertos del repo de GitHub de un proyecto: número, título, autor, texto y comentarios.")]
    async fn github_issues(&self, Parameters(a): Parameters<SlugArgs>) -> Result<String, String> {
        answer(|r| {
            let project = r.get(&a.slug)?.ok_or_else(|| anyhow::anyhow!("no existe el proyecto '{}'", a.slug))?;
            match project.repo {
                Some(repo) => atalaya_core::github::issues(&repo),
                None => Ok(serde_json::json!([])),
            }
        })
    }

    #[tool(
        description = "Controles de administración que ofrece la app de un proyecto por su puerta para Atalaya (por ejemplo: cada cliente con su plan, estado, pago y cada módulo encendido o apagado; o los anuncios). Devuelve secciones, elementos y controles con su valor actual."
    )]
    async fn admin_controls(&self, Parameters(a): Parameters<SlugArgs>) -> Result<String, String> {
        answer(|r| atalaya_core::admin::schema(r, &a.slug))
    }

    #[tool(
        description = "Cambia un control de administración de la app de un proyecto (los de admin_controls). Llega a producción de inmediato y puede cortar el acceso o cambiar el cobro de un cliente: úsalo solo cuando el usuario lo pida expresamente y confirma antes los cambios marcados con danger. Devuelve el mensaje de la app."
    )]
    async fn admin_set(&self, Parameters(a): Parameters<AdminSetArgs>) -> Result<String, String> {
        answer(|r| {
            let change = serde_json::json!({ "section": a.section, "item": a.item, "key": a.key, "value": a.value });
            Ok(Value::String(atalaya_core::admin::apply(r, &a.slug, &change, "mcp")?))
        })
    }

    #[tool(
        description = "Le encarga una consulta a otra IA del equipo que el usuario eligió en sus roles (análisis, revisión, resumen, búsqueda en el código). Solo lee: no cambia nada. Devuelve quién respondió y su respuesta. Solo funciona si el usuario encendió «Claude Code coordina»."
    )]
    async fn delegar_consulta(&self, Parameters(a): Parameters<ConsultArgs>) -> Result<String, String> {
        answer_slow(move |r| {
            let conversation = chat_conversation();
            atalaya_core::delegate::consult(r, a.ia.as_deref(), a.rol.as_deref(), &a.tarea, a.proyecto.as_deref(), conversation.as_deref())
        })
        .await
    }

    #[tool(
        description = "Le encarga un cambio a otro agente del equipo que el usuario eligió en sus roles: trabaja en la carpeta del proyecto, dentro del aislamiento de macOS, sin subir nada. Puede tardar varios minutos. Devuelve su resumen; revisa sus cambios con git diff antes de darlos por buenos. Solo funciona si el usuario encendió «Claude Code coordina»."
    )]
    async fn delegar_trabajo(&self, Parameters(a): Parameters<WorkArgs>) -> Result<String, String> {
        answer_slow(move |r| {
            let conversation = chat_conversation();
            atalaya_core::delegate::work(r, a.ia.as_deref(), &a.tarea, &a.proyecto, conversation.as_deref())
        })
        .await
    }

    #[tool(
        description = "Reparte varias consultas a la vez entre las IA del equipo que el usuario eligió en sus roles: cada una trabaja en paralelo y devuelve su respuesta. Sin ia, cada encargo va a otro del rol. Solo leen. Úsala cuando tengas varias preguntas independientes (por ejemplo, revisar varias partes o varios proyectos). Solo funciona si el usuario encendió «Claude Code coordina»."
    )]
    async fn delegar_consultas(&self, Parameters(a): Parameters<ConsultsArgs>) -> Result<String, String> {
        let orders: Vec<atalaya_core::delegate::Order> = a
            .encargos
            .into_iter()
            .map(|o| atalaya_core::delegate::Order { task: o.tarea, ai: o.ia, role: o.rol, slug: o.proyecto })
            .collect();
        answer_slow(move |r| atalaya_core::delegate::consult_many(r, &orders, chat_conversation().as_deref())).await
    }

    #[tool(
        description = "Reparte varios cambios a la vez entre los agentes del equipo que el usuario eligió en sus roles: cada uno trabaja en paralelo en la carpeta de su proyecto, dentro del aislamiento de macOS, sin subir nada. Un cambio por proyecto. Puede tardar varios minutos. Revisa cada cambio con git diff antes de darlo por bueno. Solo funciona si el usuario encendió «Claude Code coordina»."
    )]
    async fn delegar_trabajos(&self, Parameters(a): Parameters<WorksArgs>) -> Result<String, String> {
        let orders: Vec<atalaya_core::delegate::Order> = a
            .encargos
            .into_iter()
            .map(|o| atalaya_core::delegate::Order { task: o.tarea, ai: o.ia, role: None, slug: Some(o.proyecto) })
            .collect();
        answer_slow(move |r| atalaya_core::delegate::work_many(r, &orders, chat_conversation().as_deref())).await
    }

    #[tool(
        description = "Radar: cambios recientes detectados en las páginas de la competencia, con un resumen de cada uno."
    )]
    async fn radar_events(&self) -> Result<String, String> {
        answer(|r| Ok(serde_json::to_value(atalaya_core::radar::events(r)?)?))
    }
}

#[tool_handler(router = self.router)]
impl ServerHandler for Atalaya {}

pub async fn serve(read_only: bool) -> Result<()> {
    let service = Atalaya::new(read_only).serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_plain_and_the_rest_as_json() {
        let text = render(Value::String("Descartada.\nListo.".into())).unwrap();
        assert_eq!(text, "Descartada.\nListo.");
        let data = render(serde_json::json!({ "id": 4 })).unwrap();
        assert!(data.contains("\"id\": 4"));
    }

    #[test]
    fn read_only_keeps_only_what_reads() {
        let full = Atalaya::new(false).router.list_all();
        let only = Atalaya::new(true).router.list_all();
        let names: Vec<&str> = only.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names.len(), READ_ONLY.len(), "{names:?}");
        assert!(full.len() > only.len());
        for blocked in ["set_flag", "admin_set", "resolve_decision", "delegar_trabajo", "aprobar_accion", "add_improvement"] {
            assert!(!names.contains(&blocked), "{blocked}");
        }
        assert!(only.iter().all(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true)));
    }
}
