//! Conexiones: las cuentas y servicios que cada persona une a su Atalaya.
//!
//! Una conexión es un proveedor (Vercel, una base Postgres propia, un servidor, una IA…)
//! con un nombre, su configuración sin secretos y, si hace falta, una llave que vive en
//! el Llavero. Puede haber varias del mismo proveedor, y cada proyecto elige las suyas
//! con un vínculo (`project_links`): qué repo, qué hosting, qué bases.
//!
//! Las llaves que Atalaya ya usaba antes de existir las conexiones (`vercel`, `supabase`,
//! `github`…) se adoptan como conexiones predeterminadas que apuntan a la misma cuenta
//! del Llavero: no se mueven ni se borran.

use crate::Registry;
use anyhow::{Context, Result, bail};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Un campo del formulario de un proveedor.
#[derive(Debug, Clone, Serialize)]
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub required: bool,
    /// Valor que se usa si se deja vacío.
    pub default: &'static str,
    /// Lo llena Atalaya, no la persona: el formulario no lo muestra, pero lo conserva.
    pub hidden: bool,
}

const fn field(key: &'static str, label: &'static str, placeholder: &'static str, required: bool) -> Field {
    Field { key, label, placeholder, required, default: "", hidden: false }
}

const fn with_default(key: &'static str, label: &'static str, default: &'static str) -> Field {
    Field { key, label, placeholder: default, required: false, default, hidden: false }
}

const fn hidden(key: &'static str, label: &'static str) -> Field {
    Field { key, label, placeholder: "", required: false, default: "", hidden: true }
}

/// Un proveedor que Atalaya sabe usar.
#[derive(Debug, Clone, Serialize)]
pub struct Provider {
    pub id: &'static str,
    /// `git`, `hosting`, `base`, `avisos`, `ia` o `agente`.
    pub kind: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub fields: &'static [Field],
    /// Qué pide como llave; `None` si no necesita ninguna.
    pub secret: Option<&'static str>,
    /// Dónde se crea la llave.
    pub help_url: &'static str,
    /// Cuenta del Llavero que usaba Atalaya antes de las conexiones.
    #[serde(skip)]
    pub legacy_account: Option<&'static str>,
    /// Solo se admite una conexión de este proveedor (el resto de Atalaya la usa sin elegir).
    pub single: bool,
}

pub const KINDS: [(&str, &str); 6] = [
    ("git", "Repositorios"),
    ("hosting", "Hosting y servidores"),
    ("base", "Bases de datos"),
    ("avisos", "Avisos"),
    ("ia", "Modelos de IA"),
    ("agente", "Agentes de código"),
];

/// Roles que puede tener una conexión en un proyecto.
pub const ROLES: [(&str, &str); 4] = [
    ("repo", "Repositorio"),
    ("hosting", "Hosting"),
    ("base", "Base de datos"),
    ("respaldo", "Destino de respaldos"),
];

pub static PROVIDERS: &[Provider] = &[
    // --- Repositorios
    Provider {
        id: "github",
        kind: "git",
        label: "GitHub",
        description: "Repos, comprobaciones y pull requests del agente. Sin llave usa la sesión de gh.",
        fields: &[],
        secret: Some("Token personal (classic o fine-grained) con acceso a tus repos"),
        help_url: "https://github.com/settings/tokens",
        legacy_account: Some("github"),
        single: true,
    },
    Provider {
        id: "gitlab",
        kind: "git",
        label: "GitLab",
        description: "GitLab.com o tu propio GitLab: estado de los pipelines y merge requests abiertos.",
        fields: &[with_default("url", "Dirección", "https://gitlab.com")],
        secret: Some("Token de acceso personal con permiso read_api"),
        help_url: "https://gitlab.com/-/user_settings/personal_access_tokens",
        legacy_account: None,
        single: false,
    },
    Provider {
        id: "gitea",
        kind: "git",
        label: "Gitea / Forgejo",
        description: "Tu propio servidor Gitea o Forgejo (o Codeberg): estado de los commits y PR abiertos.",
        fields: &[field("url", "Dirección", "https://git.midominio.com", true)],
        secret: Some("Token de acceso con permiso de lectura de repositorios"),
        help_url: "",
        legacy_account: None,
        single: false,
    },
    // --- Hosting
    Provider {
        id: "vercel",
        kind: "hosting",
        label: "Vercel",
        description: "Despliegues de producción, interruptores y la llave de la puerta de administración.",
        fields: &[field("team_id", "Equipo (opcional)", "team_…", false)],
        secret: Some("Token de Vercel"),
        help_url: "https://vercel.com/account/settings/tokens",
        legacy_account: Some("vercel"),
        single: false,
    },
    Provider {
        id: "netlify",
        kind: "hosting",
        label: "Netlify",
        description: "Estado del último despliegue de cada sitio.",
        fields: &[],
        secret: Some("Token de acceso personal de Netlify"),
        help_url: "https://app.netlify.com/user/applications#personal-access-tokens",
        legacy_account: None,
        single: false,
    },
    Provider {
        id: "servidor",
        kind: "hosting",
        label: "Servidor propio",
        description: "Un VPS, un Proxmox o un equipo en tu casa. Revisa una URL de salud y, si das acceso SSH, \
                      la carga y el disco. Usa tus llaves SSH del sistema; Atalaya nunca las lee.",
        fields: &[
            field("health_url", "URL de salud (opcional)", "https://miapp.com/health", false),
            field("ssh_host", "Host SSH (opcional)", "192.168.1.20 o servidor.midominio.com", false),
            with_default("ssh_user", "Usuario SSH", "root"),
            with_default("ssh_port", "Puerto SSH", "22"),
        ],
        secret: None,
        help_url: "",
        legacy_account: None,
        single: false,
    },
    // --- Bases de datos
    Provider {
        id: "supabase",
        kind: "base",
        label: "Supabase",
        description: "Estado de las bases, avisos de seguridad y rendimiento, migraciones y respaldos.",
        fields: &[],
        secret: Some("Token de acceso personal de Supabase"),
        help_url: "https://supabase.com/dashboard/account/tokens",
        legacy_account: Some("supabase"),
        single: false,
    },
    Provider {
        id: "firebase",
        kind: "base",
        label: "Firebase",
        description: "Estado del proyecto, reglas de seguridad y respaldo de Firestore.",
        fields: &[],
        secret: Some("Contenido del archivo JSON de una cuenta de servicio de solo lectura"),
        help_url: "https://console.firebase.google.com/project/_/settings/serviceaccounts/adminsdk",
        legacy_account: Some("firebase"),
        single: true,
    },
    Provider {
        id: "postgres",
        kind: "base",
        label: "PostgreSQL",
        description: "Cualquier Postgres: local, en tu servidor, Neon, RDS… Revisa la conexión, el tamaño \
                      y respalda con pg_dump si está instalado.",
        fields: &[],
        secret: Some("URL de conexión: postgres://usuario:clave@host:5432/base"),
        help_url: "",
        legacy_account: None,
        single: false,
    },
    Provider {
        id: "mysql",
        kind: "base",
        label: "MySQL / MariaDB",
        description: "Revisa la conexión y el tamaño, y respalda con mysqldump si está instalado.",
        fields: &[],
        secret: Some("URL de conexión: mysql://usuario:clave@host:3306/base"),
        help_url: "",
        legacy_account: None,
        single: false,
    },
    Provider {
        id: "sqlite",
        kind: "base",
        label: "SQLite",
        description: "Un archivo de base local. Revisa su integridad y lo respalda con una copia consistente.",
        fields: &[field("path", "Archivo", "/Users/tu/proyecto/datos.db", true)],
        secret: None,
        help_url: "",
        legacy_account: None,
        single: false,
    },
    // --- Avisos
    Provider {
        id: "correo",
        kind: "avisos",
        label: "Correo",
        description: "Resumen diario y alertas por correo, desde cualquier servidor SMTP (Gmail por defecto).",
        fields: &[
            field("address", "Cuenta que envía", "tucorreo@gmail.com", true),
            field("to", "Destinatario (opcional)", "la misma cuenta", false),
            with_default("smtp_host", "Servidor SMTP", "smtp.gmail.com"),
            with_default("smtp_port", "Puerto", "465"),
            with_default("report_hour", "Hora del resumen diario", "8"),
        ],
        secret: Some("Contraseña de aplicación (en Gmail: Cuenta de Google → Seguridad → Contraseñas de aplicación)"),
        help_url: "https://myaccount.google.com/apppasswords",
        legacy_account: Some("gmail"),
        single: true,
    },
    Provider {
        id: "telegram",
        kind: "avisos",
        label: "Telegram",
        description: "Las fallas graves llegan al teléfono por un bot.",
        fields: &[field("chat", "Chat que recibe los avisos", "te lo da @userinfobot", true)],
        secret: Some("Token del bot (te lo da @BotFather)"),
        help_url: "https://t.me/BotFather",
        legacy_account: Some("telegram"),
        single: true,
    },
    // --- IA
    Provider {
        id: "anthropic",
        kind: "ia",
        label: "Anthropic (Claude)",
        description: "Modelos Claude por API.",
        fields: &[with_default("model", "Modelo predeterminado", "claude-sonnet-5-5")],
        secret: Some("Llave de API (sk-ant-…)"),
        help_url: "https://console.anthropic.com/settings/keys",
        legacy_account: None,
        single: false,
    },
    Provider {
        id: "anthropic-admin",
        kind: "ia",
        label: "Claude Platform (consumo)",
        description: "Lee el gasto y uso de la organización en Claude Platform. No sirve para llamar modelos.",
        fields: &[],
        secret: Some("Llave de administrador (sk-ant-admin…), de Settings → Admin keys"),
        help_url: "https://console.anthropic.com/settings/admin-keys",
        legacy_account: Some("claude"),
        single: true,
    },
    Provider {
        id: "openai",
        kind: "ia",
        label: "OpenAI",
        description: "Modelos GPT y o-series por API.",
        fields: &[field("model", "Modelo predeterminado", "el que uses, por ejemplo gpt-5", false)],
        secret: Some("Llave de API (sk-…)"),
        help_url: "https://platform.openai.com/api-keys",
        legacy_account: None,
        single: false,
    },
    Provider {
        id: "gemini",
        kind: "ia",
        label: "Google Gemini",
        description: "Modelos Gemini por la API de Google AI Studio.",
        fields: &[field("model", "Modelo predeterminado", "el que uses, por ejemplo gemini-2.5-pro", false)],
        secret: Some("Llave de API de Google AI Studio"),
        help_url: "https://aistudio.google.com/apikey",
        legacy_account: None,
        single: false,
    },
    Provider {
        id: "openai-compatible",
        kind: "ia",
        label: "Compatible con OpenAI",
        description: "OpenRouter, DeepSeek, Groq, Mistral, xAI, Together o cualquier servicio con la misma API.",
        fields: &[
            field("base_url", "Dirección de la API", "https://openrouter.ai/api/v1", true),
            field("model", "Modelo predeterminado", "el que uses, por ejemplo deepseek/deepseek-chat", false),
        ],
        secret: Some("Llave de API del servicio"),
        help_url: "",
        legacy_account: None,
        single: false,
    },
    Provider {
        id: "ollama",
        kind: "ia",
        label: "Modelos locales (Ollama, LM Studio, Jan, llama.cpp)",
        description: "Modelos que corren en tu equipo o tu red, sin costo por token ni llave.",
        fields: &[
            with_default("base_url", "Dirección", "http://127.0.0.1:11434/v1"),
            field("model", "Modelo predeterminado", "el que tengas descargado, por ejemplo qwen2.5-coder", false),
        ],
        secret: None,
        help_url: "https://ollama.com",
        legacy_account: None,
        single: false,
    },
    // --- Agentes de código
    Provider {
        id: "claude-code",
        kind: "agente",
        label: "Claude Code",
        description: "Agente de terminal de Anthropic. Trabaja con su propio aislamiento.",
        fields: &[],
        secret: None,
        help_url: "https://docs.claude.com/claude-code",
        legacy_account: None,
        single: true,
    },
    // Los demás agentes corren dentro del encierro de Atalaya (`sandbox-exec`), que decide
    // qué pueden escribir; por eso se lanzan sin su propio encierro, que no se puede anidar.
    // `command` es para trabajar (cambiar código); `ask_command`, para responder.
    agent(
        "codex",
        "Codex CLI",
        "Agente de OpenAI. Usa tu suscripción de ChatGPT o una llave de OpenAI.",
        "https://github.com/openai/codex",
        &[
            with_default("command", "Comando para trabajar", "codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox {tarea}"),
            with_default("ask_command", "Comando para responder", "codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox {tarea}"),
        ],
    ),
    agent(
        "antigravity",
        "Antigravity CLI",
        "Agente de Google. Usa tu cuenta de Google (gratis, Google AI Pro o Ultra).",
        "https://antigravity.google/docs/cli/",
        &[
            with_default("command", "Comando para trabajar", "agy -p {tarea} --dangerously-skip-permissions"),
            with_default("ask_command", "Comando para responder", "agy -p {tarea} --mode plan"),
        ],
    ),
    Provider {
        // En modo automático Gemini CLI solo lee la llave de GEMINI_API_KEY: Atalaya se la
        // entrega desde el Llavero. Con Vertex AI o cuenta de empresa no hace falta.
        secret: Some("Llave de Gemini (opcional; solo si usas Gemini CLI con llave de Google AI Studio)"),
        help_url: "https://aistudio.google.com/apikey",
        ..agent(
            "gemini-cli",
            "Gemini CLI",
            "Agente de Google para llaves de Gemini, Vertex AI o cuentas de empresa. Con cuenta personal, usa Antigravity CLI.",
            "https://github.com/google-gemini/gemini-cli",
            &[
                // --skip-trust: sin él, en una carpeta nueva Gemini baja a «preguntar» y no hace nada.
                with_default("command", "Comando para trabajar", "gemini --skip-trust --yolo -p {tarea}"),
                with_default("ask_command", "Comando para responder", "gemini --skip-trust --approval-mode plan -p {tarea}"),
            ],
        )
    },
    agent(
        "qwen-code",
        "Qwen Code",
        "Agente de Alibaba. Gratis con un modelo de tu Mac (Ollama, LM Studio) o con una llave de un servicio; también con tu suscripción de Qwen.",
        "https://github.com/QwenLM/qwen-code",
        &[
            with_default("command", "Comando para trabajar", "qwen --yolo -p {tarea}"),
            with_default("ask_command", "Comando para responder", "qwen --yolo -p {tarea}"),
            // Con estos dos, usa ese modelo en vez de la cuenta (ver `agents::OWN_MODELS`).
            field("model_base_url", "Dirección del modelo (opcional)", "vacío: tu cuenta de Qwen; de este Mac: http://127.0.0.1:11434/v1", false),
            field("model", "Modelo (opcional)", "por ejemplo qwen3.5:4b", false),
            hidden("model_key", "Llave del modelo (cuenta del Llavero)"),
        ],
    ),
    agent(
        "kimi",
        "Kimi Code",
        "Agente de Moonshot. Usa tu cuenta de Kimi.",
        "https://www.kimi.com/code/docs/en/kimi-code-cli/",
        &[
            // `-p` ya decide solo (no se combina con --plan): el encierro protege los proyectos.
            with_default("command", "Comando para trabajar", "kimi -p {tarea}"),
            with_default("ask_command", "Comando para responder", "kimi -p {tarea}"),
        ],
    ),
    agent(
        "copilot",
        "GitHub Copilot CLI",
        "Agente de GitHub. Usa tu suscripción de Copilot.",
        "https://github.com/github/copilot-cli",
        &[
            // -s: solo la respuesta, sin avisos como «Copilot Fusion selected…».
            with_default("command", "Comando para trabajar", "copilot -s --allow-all -p {tarea}"),
            with_default("ask_command", "Comando para responder", "copilot -s --allow-all-tools -p {tarea}"),
        ],
    ),
    agent(
        "cursor",
        "Cursor CLI",
        "Agente de Cursor. Usa tu suscripción de Cursor.",
        "https://cursor.com/cli",
        &[
            with_default("command", "Comando para trabajar", "cursor-agent --print --force --output-format text {tarea}"),
            with_default("ask_command", "Comando para responder", "cursor-agent --print --trust --mode ask --output-format text {tarea}"),
        ],
    ),
    agent(
        "opencode",
        "OpenCode",
        "Agente abierto que funciona con muchos proveedores y suscripciones.",
        "https://opencode.ai",
        &[
            with_default("command", "Comando para trabajar", "opencode run {tarea}"),
            with_default("ask_command", "Comando para responder", "opencode run {tarea}"),
        ],
    ),
    agent(
        "droid",
        "Factory Droid",
        "Agente de Factory. Usa tu cuenta de Factory.",
        "https://factory.ai",
        &[
            with_default("command", "Comando para trabajar", "droid exec --auto high {tarea}"),
            with_default("ask_command", "Comando para responder", "droid exec {tarea}"),
        ],
    ),
    agent(
        "amp",
        "Amp",
        "Agente de Sourcegraph. Usa tu cuenta de Amp.",
        "https://ampcode.com",
        &[
            with_default("command", "Comando para trabajar", "amp --dangerously-allow-all -x {tarea}"),
            with_default("ask_command", "Comando para responder", "amp --dangerously-allow-all -x {tarea}"),
        ],
    ),
    Provider {
        id: "agente-personalizado",
        kind: "agente",
        label: "Agente personalizado",
        description: "Cualquier otro agente de terminal. La plantilla recibe la tarea en {tarea} y, si el agente acepta un archivo de MCP, el de Atalaya (solo lectura) en {mcp_config}.",
        fields: &[
            field("command", "Comando para trabajar", "aider --yes --message {tarea}", true),
            field("ask_command", "Comando para responder (opcional)", "el mismo de trabajar", false),
        ],
        secret: None,
        help_url: "",
        legacy_account: None,
        single: false,
    },
];

const fn agent(
    id: &'static str,
    label: &'static str,
    description: &'static str,
    help_url: &'static str,
    fields: &'static [Field],
) -> Provider {
    Provider { id, kind: "agente", label, description, fields, secret: None, help_url, legacy_account: None, single: false }
}

/// Servicios con la misma API de OpenAI: elegir uno llena la dirección.
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub base_url: &'static str,
    /// Dónde se crea la llave.
    pub key_url: &'static str,
    pub model_hint: &'static str,
    /// Qué se puede usar gratis, si algo (créditos de regalo que no se renuevan no cuentan).
    pub free: Option<&'static str>,
    /// Qué cambiar en la dirección, si lleva un dato de la cuenta (como `{cuenta}` en Cloudflare).
    pub note: Option<&'static str>,
}

/// Lo que hay que reemplazar en la dirección de un servicio que la arma con un dato de la cuenta.
pub const ACCOUNT_PLACEHOLDER: &str = "{cuenta}";

const fn preset(id: &'static str, label: &'static str, base_url: &'static str, key_url: &'static str, model_hint: &'static str) -> Preset {
    Preset { id, label, base_url, key_url, model_hint, free: None, note: None }
}

const fn with_note(note: &'static str, p: Preset) -> Preset {
    Preset { note: Some(note), ..p }
}

const fn free(note: &'static str, p: Preset) -> Preset {
    Preset { free: Some(note), ..p }
}

/// Los que tienen uso gratis van primero. Revisado en noviembre de 2026; cambia seguido.
pub static API_PRESETS: &[Preset] = &[
    free(
        "modelos «:free» sin costo (50 pedidos al día; 1000 si compraste créditos)",
        preset("openrouter", "OpenRouter (cientos de modelos)", "https://openrouter.ai/api/v1", "https://openrouter.ai/keys", "uno de su lista que termine en «:free»"),
    ),
    free(
        "plan Free sin tarjeta; límites por modelo (en gpt-oss-120b, 30 pedidos por minuto y 1000 al día)",
        preset("groq", "Groq", "https://api.groq.com/openai/v1", "https://console.groq.com/keys", "openai/gpt-oss-120b"),
    ),
    free(
        "plan Experiment gratis, sin tarjeta (puede usar tus pedidos para entrenar)",
        preset("mistral", "Mistral", "https://api.mistral.ai/v1", "https://console.mistral.ai/api-keys", "mistral-large-latest"),
    ),
    // GitHub Models se retiró el 30 de julio de 2026: ya no responde.
    free(
        "plan Free sin tarjeta: 20 pedidos al día y 200 000 tokens por modelo",
        preset("sambanova", "SambaNova", "https://api.sambanova.ai/v1", "https://cloud.sambanova.ai/apis", "gpt-oss-120b"),
    ),
    free(
        "10 000 «neuronas» al día entre todos los modelos (rinden mucho más en los chicos)",
        with_note(
            "En la dirección, cambia {cuenta} por el ID de tu cuenta de Cloudflare (está en el panel, en Workers AI). La llave es un token de API con permiso de Workers AI.",
            preset(
                "cloudflare",
                "Cloudflare Workers AI",
                "https://api.cloudflare.com/client/v4/accounts/{cuenta}/ai/v1",
                "https://dash.cloudflare.com/profile/api-tokens",
                "@cf/openai/gpt-oss-120b",
            ),
        ),
    ),
    free(
        "uso de prueba sin costo, limitado por minuto (unos 40 pedidos; NVIDIA no publica la cifra)",
        preset("nvidia", "NVIDIA NIM", "https://integrate.api.nvidia.com/v1", "https://build.nvidia.com", "meta/llama-3.3-70b-instruct"),
    ),
    free("modelos GLM Flash sin costo", preset("zai", "Z.ai (GLM)", "https://api.z.ai/api/paas/v4", "https://z.ai/manage-apikey/apikey-list", "glm-4.7-flash")),
    preset("deepseek", "DeepSeek", "https://api.deepseek.com/v1", "https://platform.deepseek.com/api_keys", "deepseek-chat"),
    preset("xai", "xAI (Grok)", "https://api.x.ai/v1", "https://console.x.ai", "grok-4"),
    preset("moonshot", "Moonshot (Kimi)", "https://api.moonshot.ai/v1", "https://platform.moonshot.ai/console/api-keys", "kimi-k2-turbo-preview"),
    preset("qwen", "Alibaba (Qwen)", "https://dashscope-intl.aliyuncs.com/compatible-mode/v1", "https://modelstudio.console.alibabacloud.com", "qwen-plus"),
    preset("minimax", "MiniMax", "https://api.minimax.io/v1", "https://www.minimax.io/platform", "MiniMax-M2"),
    preset("together", "Together AI", "https://api.together.xyz/v1", "https://api.together.ai/settings/api-keys", "meta-llama/Llama-3.3-70B-Instruct-Turbo"),
    preset("fireworks", "Fireworks", "https://api.fireworks.ai/inference/v1", "https://fireworks.ai/account/api-keys", "accounts/fireworks/models/llama-v3p3-70b-instruct"),
    // Sin plan gratis permanente: US$ 5 de prueba por 30 días, con tarjeta.
    preset("cerebras", "Cerebras", "https://api.cerebras.ai/v1", "https://cloud.cerebras.ai", "gpt-oss-120b"),
    preset("perplexity", "Perplexity", "https://api.perplexity.ai", "https://www.perplexity.ai/settings/api", "sonar-pro"),
    preset("huggingface", "Hugging Face", "https://router.huggingface.co/v1", "https://huggingface.co/settings/tokens", "openai/gpt-oss-120b"),
    preset("vercel-gateway", "Vercel AI Gateway", "https://ai-gateway.vercel.sh/v1", "https://vercel.com/dashboard/ai-gateway", "anthropic/claude-sonnet-5.5"),
    preset("cohere", "Cohere", "https://api.cohere.ai/compatibility/v1", "https://dashboard.cohere.com/api-keys", "command-a-03-2025"),
];

/// Proveedores de modelos con uso gratis (los servicios compatibles están en `API_PRESETS`).
const FREE_PROVIDERS: &[(&str, &str)] = &[
    ("ollama", "gratis: corre en tu equipo (los modelos «:cloud» de Ollama se cobran)"),
    ("gemini", "capa gratis de Google AI Studio en los modelos Flash, con límites"),
];

/// Qué tiene gratis un proveedor de modelos, si algo.
pub fn free_note(provider_id: &str) -> Option<&'static str> {
    FREE_PROVIDERS.iter().find(|(id, _)| *id == provider_id).map(|(_, note)| *note)
}

/// Un modelo «:cloud» de Ollama: se pide al servidor de este Mac, pero corre en la nube de
/// Ollama y se cobra por uso (noviembre de 2026: sin capa gratis, solo créditos de los
/// planes Pro y Max).
pub fn is_cloud_model(model: &str) -> bool {
    model.ends_with(":cloud") || model.ends_with("-cloud")
}

/// El servicio compatible con OpenAI que corresponde a una dirección.
pub fn preset_of(base_url: &str) -> Option<&'static Preset> {
    let url = base_url.trim().trim_end_matches('/');
    API_PRESETS.iter().find(|p| {
        let pattern = p.base_url.trim_end_matches('/');
        match pattern.split_once(ACCOUNT_PLACEHOLDER) {
            // El dato de la cuenta: algo sin «/» ni llaves entre el comienzo y el final.
            Some((start, end)) => url.strip_prefix(start).and_then(|rest| rest.strip_suffix(end)).is_some_and(|account| {
                !account.is_empty() && !account.contains(['/', '{', '}'])
            }),
            None => pattern == url,
        }
    })
}

/// Si usar esta conexión de modelos puede salir gratis. En OpenRouter, solo sus modelos «:free».
pub fn is_free(c: &Connection) -> bool {
    if c.kind != "ia" {
        return false;
    }
    if c.provider == "ollama" {
        return !is_cloud_model(&c.config_str("model").unwrap_or_default());
    }
    if free_note(&c.provider).is_some() {
        return true;
    }
    let base = match c.provider.as_str() {
        "openai-compatible" => c.config_str("base_url"),
        _ => None,
    };
    base.is_some_and(|b| is_free_endpoint(&b, &c.config_str("model").unwrap_or_default()))
}

/// Si un modelo en esta dirección puede usarse gratis: uno de este Mac, o un servicio con
/// uso gratis (en OpenRouter, solo sus modelos «:free»).
pub fn is_free_endpoint(base_url: &str, model: &str) -> bool {
    if ["://127.0.0.1", "://localhost", "://[::1]"].iter().any(|h| base_url.contains(h)) {
        return !is_cloud_model(model);
    }
    match preset_of(base_url) {
        Some(p) if p.id == "openrouter" => model.ends_with(":free"),
        Some(p) => p.free.is_some(),
        None => false,
    }
}

pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// La llave guardada en una cuenta del Llavero. Nunca sale de Atalaya.
pub fn secret_of_account(account: &str) -> Option<String> {
    // Las llaves adoptadas se leen como siempre; las nuevas pueden traer cualquier texto.
    if account.starts_with("conn:") { crate::secrets::get_text(account) } else { crate::secrets::get(account) }
}

/// Una conexión guardada.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub kind: String,
    pub provider: String,
    pub name: String,
    pub config: Value,
    /// Cuenta del Llavero donde está su llave.
    pub secret_account: Option<String>,
    pub created_at: String,
}

impl Connection {
    /// Un valor de la configuración, o el valor por defecto del campo.
    pub fn config_str(&self, key: &str) -> Option<String> {
        let saved = self.config[key].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
        saved.or_else(|| {
            provider(&self.provider)?
                .fields
                .iter()
                .find(|f| f.key == key && !f.default.is_empty())
                .map(|f| f.default.to_string())
        })
    }

    /// La llave, si tiene. Nunca sale de Atalaya: solo se usa para hablar con el servicio.
    pub fn secret(&self) -> Option<String> {
        secret_of_account(self.secret_account.as_deref()?)
    }
}

/// Un vínculo entre un proyecto y una conexión.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Link {
    pub slug: String,
    pub role: String,
    pub connection_id: String,
    /// Lo propio del proyecto en ese servicio: la referencia de Supabase, el sitio de
    /// Netlify, el repo en GitLab, el nombre de la base…
    pub config: Value,
}

impl Link {
    pub fn config_str(&self, key: &str) -> Option<String> {
        self.config[key].as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
    }
}

pub(crate) fn init(registry: &Registry) -> Result<()> {
    registry.conn().execute_batch(
        "CREATE TABLE IF NOT EXISTS connections (
             id             TEXT PRIMARY KEY,
             kind           TEXT NOT NULL,
             provider       TEXT NOT NULL,
             name           TEXT NOT NULL,
             config         TEXT NOT NULL DEFAULT '{}',
             secret_account TEXT,
             created_at     TEXT NOT NULL DEFAULT (datetime('now'))
         );
         CREATE TABLE IF NOT EXISTS project_links (
             slug          TEXT NOT NULL,
             role          TEXT NOT NULL,
             connection_id TEXT NOT NULL,
             config        TEXT NOT NULL DEFAULT '{}',
             PRIMARY KEY (slug, role, connection_id)
         );",
    )?;
    Ok(())
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Connection> {
    let config: String = r.get(4)?;
    Ok(Connection {
        id: r.get(0)?,
        kind: r.get(1)?,
        provider: r.get(2)?,
        name: r.get(3)?,
        config: serde_json::from_str(&config).unwrap_or_else(|_| json!({})),
        secret_account: r.get(5)?,
        created_at: r.get(6)?,
    })
}

const COLUMNS: &str = "id, kind, provider, name, config, secret_account, created_at";

pub fn all(registry: &Registry) -> Result<Vec<Connection>> {
    let mut stmt = registry
        .conn()
        .prepare(&format!("SELECT {COLUMNS} FROM connections ORDER BY kind, provider, created_at"))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn get(registry: &Registry, id: &str) -> Result<Option<Connection>> {
    Ok(registry
        .conn()
        .query_row(&format!("SELECT {COLUMNS} FROM connections WHERE id = ?1"), [id], from_row)
        .optional()?)
}

/// Conexiones de un proveedor, la más antigua primero (la predeterminada).
pub fn of_provider(registry: &Registry, provider_id: &str) -> Result<Vec<Connection>> {
    Ok(all(registry)?.into_iter().filter(|c| c.provider == provider_id).collect())
}

/// La conexión que se usa cuando un proyecto no eligió otra.
pub fn default_for(registry: &Registry, provider_id: &str) -> Result<Option<Connection>> {
    Ok(of_provider(registry, provider_id)?.into_iter().next())
}

/// Convierte en conexiones las llaves de antes de que existieran. Solo crea la fila: la
/// llave queda donde estaba. Corre al listar, así una llave guardada después desde la
/// terminal (`atalaya secret set vercel`) también aparece.
pub fn adopt_legacy(registry: &Registry) -> Result<usize> {
    let mut adopted = 0;
    for p in PROVIDERS {
        let Some(account) = p.legacy_account else { continue };
        if !of_provider(registry, p.id)?.is_empty() || registry.setting(&dismissed_key(p.id))?.is_some() {
            continue;
        }
        let mut config = json!({});
        let has_secret = crate::secrets::get(account).is_some();
        // El correo y Telegram guardaban parte de su configuración como ajustes.
        match p.id {
            "correo" => {
                for (key, setting) in [("address", "email_address"), ("to", "email_to"), ("report_hour", "report_hour")] {
                    if let Some(v) = registry.setting(setting)? {
                        config[key] = json!(v);
                    }
                }
            }
            "telegram" => {
                if let Some(v) = registry.setting("telegram_chat")? {
                    config["chat"] = json!(v);
                }
            }
            "vercel" => {
                if let Some(v) = registry.setting("flags_team_id")? {
                    config["team_id"] = json!(v);
                }
            }
            _ => {}
        }
        if !has_secret {
            continue;
        }
        registry.conn().execute(
            "INSERT INTO connections (id, kind, provider, name, config, secret_account) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![unique_id(registry, p.id)?, p.kind, p.id, p.label, config.to_string(), account],
        )?;
        adopted += 1;
    }
    Ok(adopted)
}

/// Ajuste que recuerda que la persona quitó la conexión adoptada de un proveedor, para
/// no volver a crearla sola.
fn dismissed_key(provider_id: &str) -> String {
    format!("conn_legacy_quitada:{provider_id}")
}

fn unique_id(registry: &Registry, base: &str) -> Result<String> {
    let base = crate::registry::slugify(base);
    let base = if base.is_empty() { "conexion".to_string() } else { base };
    let mut id = base.clone();
    let mut n = 2;
    while get(registry, &id)?.is_some() {
        id = format!("{base}-{n}");
        n += 1;
    }
    Ok(id)
}

/// Lo que llega del formulario para crear o editar una conexión.
#[derive(Debug, Clone, Deserialize)]
pub struct Input {
    /// Vacío para crear una nueva.
    #[serde(default)]
    pub id: Option<String>,
    pub provider: String,
    pub name: String,
    #[serde(default)]
    pub config: Value,
    /// Llave nueva. Vacía o ausente deja la que había.
    #[serde(default)]
    pub secret: Option<String>,
}

/// Crea o actualiza una conexión. La llave se guarda en el Llavero; la base solo guarda
/// en qué cuenta del Llavero está.
pub fn save(registry: &Registry, input: &Input) -> Result<Connection> {
    let p = provider(&input.provider).with_context(|| format!("proveedor desconocido '{}'", input.provider))?;
    let name = input.name.trim();
    let name = if name.is_empty() { p.label } else { name };

    // Solo los campos del proveedor, recortados.
    let mut config = json!({});
    for f in p.fields {
        let value = input.config[f.key].as_str().map(str::trim).unwrap_or_default();
        if value.is_empty() {
            if f.required {
                bail!("falta «{}»", f.label);
            }
            continue;
        }
        if (f.key.ends_with("url") || f.key == "url") && !value.starts_with("http://") && !value.starts_with("https://") {
            bail!("«{}» debe empezar con http:// o https://", f.label);
        }
        if value.contains(ACCOUNT_PLACEHOLDER) {
            bail!("en «{}», cambia {ACCOUNT_PLACEHOLDER} por el ID de tu cuenta", f.label);
        }
        config[f.key] = json!(value);
    }

    let existing = match input.id.as_deref().filter(|s| !s.is_empty()) {
        Some(id) => Some(get(registry, id)?.with_context(|| format!("no existe la conexión '{id}'"))?),
        None => None,
    };
    if existing.is_none() && p.single && !of_provider(registry, p.id)?.is_empty() {
        bail!("{} admite una sola conexión; edita la que ya existe", p.label);
    }
    let secret = input.secret.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(s) = secret {
        validate_secret(p.id, s)?;
    }
    if existing.as_ref().is_none_or(|c| c.secret_account.is_none()) && p.secret.is_some() && secret.is_none() {
        // GitHub funciona sin llave (usa gh); el resto la necesita.
        if p.id != "github" {
            bail!("falta la llave: {}", p.secret.unwrap_or_default());
        }
    }

    let id = match &existing {
        Some(c) => c.id.clone(),
        None => unique_id(registry, if p.single { p.id } else { name })?,
    };
    let account = existing
        .as_ref()
        .and_then(|c| c.secret_account.clone())
        .or_else(|| p.legacy_account.filter(|_| p.single).map(String::from))
        .or_else(|| p.secret.map(|_| format!("conn:{id}")));

    if let (Some(value), Some(account)) = (secret, account.as_deref()) {
        let saved = if account.starts_with("conn:") {
            crate::secrets::set_text(account, value)?
        } else if p.id == "firebase" {
            // Firebase guarda el JSON en base64, como siempre lo hizo.
            use base64::Engine;
            crate::secrets::set(account, &base64::engine::general_purpose::STANDARD.encode(value))?
        } else {
            crate::secrets::set(account, value)?
        };
        if !saved {
            bail!("no se pudo guardar la llave en el Llavero");
        }
    }

    registry.conn().execute(
        "INSERT INTO connections (id, kind, provider, name, config, secret_account) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, config = excluded.config,
                                       secret_account = excluded.secret_account",
        params![id, p.kind, p.id, name, config.to_string(), account.filter(|_| p.secret.is_some())],
    )?;
    // Los ajustes de antes siguen siendo la fuente de algunos módulos: se mantienen al día.
    sync_legacy_settings(registry, p.id, &config)?;
    registry.record_event("atalaya", &format!("conexión guardada: {name} ({})", p.label), "conexiones")?;
    get(registry, &id)?.context("la conexión no quedó guardada")
}

fn sync_legacy_settings(registry: &Registry, provider_id: &str, config: &Value) -> Result<()> {
    let pairs: &[(&str, &str)] = match provider_id {
        "correo" => &[("address", "email_address"), ("to", "email_to"), ("report_hour", "report_hour")],
        "telegram" => &[("chat", "telegram_chat")],
        _ => &[],
    };
    for (key, setting) in pairs {
        if let Some(v) = config[*key].as_str() {
            registry.set_setting(setting, v)?;
        }
    }
    Ok(())
}

/// Revisa la forma de la llave antes de guardarla, para atajar pegados equivocados.
fn validate_secret(provider_id: &str, value: &str) -> Result<()> {
    match provider_id {
        "postgres" if !(value.starts_with("postgres://") || value.starts_with("postgresql://")) => {
            bail!("la URL de Postgres debe empezar con postgres:// o postgresql://")
        }
        "mysql" if !(value.starts_with("mysql://") || value.starts_with("mariadb://")) => {
            bail!("la URL de MySQL debe empezar con mysql://")
        }
        "firebase" => {
            let account: Value = serde_json::from_str(value).context("pega el contenido completo del archivo JSON")?;
            if account["type"] != "service_account" || !account["private_key"].is_string() {
                bail!("eso no es una llave de cuenta de servicio de Google");
            }
            Ok(())
        }
        "anthropic-admin" if !value.starts_with("sk-ant-admin") => {
            bail!("la llave de administrador empieza con sk-ant-admin")
        }
        _ if value.chars().any(char::is_whitespace) && provider_id != "firebase" => {
            bail!("la llave tiene espacios; revisa que se haya copiado completa")
        }
        _ => Ok(()),
    }
}

/// Quita la conexión y sus vínculos. La llave del Llavero solo se borra si se pide
/// expresamente (`forget_secret`), y nunca la de una cuenta adoptada de antes.
pub fn remove(registry: &Registry, id: &str, forget_secret: bool) -> Result<()> {
    let c = get(registry, id)?.with_context(|| format!("no existe la conexión '{id}'"))?;
    registry.conn().execute("DELETE FROM project_links WHERE connection_id = ?1", [id])?;
    registry.conn().execute("DELETE FROM connections WHERE id = ?1", [id])?;
    if c.secret_account.as_deref().is_some_and(|a| !a.starts_with("conn:")) {
        registry.set_setting(&dismissed_key(&c.provider), "1")?;
    }
    if forget_secret {
        if let Some(account) = c.secret_account.as_deref().filter(|a| a.starts_with("conn:")) {
            let _ = crate::secrets::remove(account);
        }
    }
    registry.record_event("atalaya", &format!("conexión quitada: {}", c.name), "conexiones")?;
    Ok(())
}

/// Resultado de la última prueba de una conexión.
pub fn last_test(registry: &Registry, id: &str) -> Result<Value> {
    Ok(registry
        .setting(&format!("conn_test:{id}"))?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null))
}

/// Prueba la conexión contra su servicio y guarda el resultado.
pub fn test(registry: &Registry, id: &str) -> Result<String> {
    let c = get(registry, id)?.with_context(|| format!("no existe la conexión '{id}'"))?;
    let outcome = crate::providers::test(registry, &c);
    let record = match &outcome {
        Ok(message) => json!({ "ok": true, "message": message, "at": registry.utc_now()? }),
        Err(e) => json!({ "ok": false, "message": format!("{e:#}"), "at": registry.utc_now()? }),
    };
    registry.set_setting(&format!("conn_test:{id}"), &record.to_string())?;
    outcome
}

/// Todas las conexiones para mostrar: sin llaves, con su estado y qué proyectos las usan.
pub fn list(registry: &Registry) -> Result<Value> {
    adopt_legacy(registry)?;
    let links = all_links(registry)?;
    let items: Vec<Value> = all(registry)?
        .into_iter()
        .map(|c| {
            let used_by: Vec<&str> =
                links.iter().filter(|l| l.connection_id == c.id).map(|l| l.slug.as_str()).collect();
            let has_secret = c.secret_account.is_some() && c.secret().is_some();
            json!({
                "id": c.id,
                "kind": c.kind,
                "provider": c.provider,
                "name": c.name,
                "config": c.config,
                "has_secret": has_secret,
                "legacy": c.secret_account.as_deref().is_some_and(|a| !a.starts_with("conn:")),
                "last_test": last_test(registry, &c.id).unwrap_or(Value::Null),
                "used_by": used_by,
                "created_at": c.created_at,
            })
        })
        .collect();
    Ok(json!({
        "connections": items,
        "providers": PROVIDERS,
        "kinds": KINDS.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
        "roles": ROLES.iter().map(|(id, label)| json!({ "id": id, "label": label })).collect::<Vec<_>>(),
    }))
}

/* ------------------------------------------------------------------ Vínculos */

fn link_from_row(r: &rusqlite::Row) -> rusqlite::Result<Link> {
    let config: String = r.get(3)?;
    Ok(Link {
        slug: r.get(0)?,
        role: r.get(1)?,
        connection_id: r.get(2)?,
        config: serde_json::from_str(&config).unwrap_or_else(|_| json!({})),
    })
}

pub fn all_links(registry: &Registry) -> Result<Vec<Link>> {
    let mut stmt = registry
        .conn()
        .prepare("SELECT slug, role, connection_id, config FROM project_links ORDER BY slug, role")?;
    let rows = stmt.query_map([], link_from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn links(registry: &Registry, slug: &str) -> Result<Vec<Link>> {
    Ok(all_links(registry)?.into_iter().filter(|l| l.slug == slug).collect())
}

/// Vínculos de un proyecto con su conexión, filtrados por rol y opcionalmente por proveedor.
pub fn linked(registry: &Registry, slug: &str, role: &str, provider_id: Option<&str>) -> Result<Vec<(Link, Connection)>> {
    let mut out = Vec::new();
    for l in links(registry, slug)?.into_iter().filter(|l| l.role == role) {
        if let Some(c) = get(registry, &l.connection_id)? {
            if provider_id.is_none_or(|p| p == c.provider) {
                out.push((l, c));
            }
        }
    }
    Ok(out)
}

/// Une un proyecto con una conexión en un rol. Si ya estaba, actualiza su configuración.
pub fn link(registry: &Registry, slug: &str, role: &str, connection_id: &str, config: &Value) -> Result<()> {
    registry.get(slug)?.with_context(|| format!("no existe el proyecto '{slug}'"))?;
    let c = get(registry, connection_id)?.with_context(|| format!("no existe la conexión '{connection_id}'"))?;
    if !ROLES.iter().any(|(id, _)| *id == role) {
        bail!("rol inválido '{role}'; usa {}", ROLES.iter().map(|(id, _)| *id).collect::<Vec<_>>().join(", "));
    }
    let fits = match role {
        "repo" => c.kind == "git",
        "hosting" => c.kind == "hosting",
        "base" | "respaldo" => c.kind == "base",
        _ => false,
    };
    if !fits {
        bail!("una conexión de tipo '{}' no sirve como {role}", c.kind);
    }
    let config = if config.is_object() { config.clone() } else { json!({}) };
    registry.conn().execute(
        "INSERT INTO project_links (slug, role, connection_id, config) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(slug, role, connection_id) DO UPDATE SET config = excluded.config",
        params![slug, role, connection_id, config.to_string()],
    )?;
    registry.record_event(slug, &format!("vinculado a {} como {role}", c.name), "conexiones")?;
    Ok(())
}

pub fn unlink(registry: &Registry, slug: &str, role: &str, connection_id: &str) -> Result<bool> {
    let n = registry.conn().execute(
        "DELETE FROM project_links WHERE slug = ?1 AND role = ?2 AND connection_id = ?3",
        params![slug, role, connection_id],
    )?;
    if n > 0 {
        registry.record_event(slug, &format!("desvinculado de {connection_id} ({role})"), "conexiones")?;
    }
    Ok(n > 0)
}

/// La llave con que se habla con un proveedor para un proyecto: la de la conexión
/// vinculada al proyecto si hay una de ese proveedor, si no la predeterminada, y si
/// tampoco existe, la cuenta de antes del Llavero.
pub fn token_for(registry: &Registry, slug: Option<&str>, provider_id: &str) -> Option<String> {
    if let Some(slug) = slug {
        for role in ["hosting", "base", "repo", "respaldo"] {
            if let Ok(found) = linked(registry, slug, role, Some(provider_id)) {
                if let Some(secret) = found.iter().find_map(|(_, c)| c.secret()) {
                    return Some(secret);
                }
            }
        }
    }
    if let Ok(Some(c)) = default_for(registry, provider_id) {
        if let Some(secret) = c.secret() {
            return Some(secret);
        }
    }
    provider(provider_id).and_then(|p| p.legacy_account).and_then(crate::secrets::get)
}

/// La llave predeterminada de un proveedor, sin proyecto.
pub fn token_for_default(provider_id: &str) -> Option<String> {
    let registry = Registry::open_default().ok()?;
    token_for(&registry, None, provider_id)
}

/// Ve si un vínculo propuesto ya existe, para no repetir propuestas.
pub fn has_link(registry: &Registry, slug: &str, role: &str, connection_id: &str) -> Result<bool> {
    Ok(registry
        .conn()
        .query_row(
            "SELECT count(*) > 0 FROM project_links WHERE slug = ?1 AND role = ?2 AND connection_id = ?3",
            params![slug, role, connection_id],
            |r| r.get(0),
        )
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(name: &str) -> Registry {
        let dir = std::env::temp_dir().join(format!("atalaya-conn-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Registry::open(&dir.join("a.db")).unwrap()
    }

    #[test]
    fn every_provider_has_a_known_kind_and_unique_id() {
        for p in PROVIDERS {
            assert!(KINDS.iter().any(|(k, _)| *k == p.kind), "{} tiene un tipo desconocido", p.id);
            assert_eq!(PROVIDERS.iter().filter(|o| o.id == p.id).count(), 1, "{} repetido", p.id);
        }
    }

    #[test]
    fn presets_recognize_their_address_and_what_is_free() {
        // Cloudflare lleva el ID de la cuenta en la dirección.
        let cf = "https://api.cloudflare.com/client/v4/accounts/0a1b2c3d4e/ai/v1/";
        assert_eq!(preset_of(cf).map(|p| p.id), Some("cloudflare"));
        assert!(preset_of("https://api.cloudflare.com/client/v4/accounts/{cuenta}/ai/v1").is_none());
        assert!(preset_of("https://api.cloudflare.com/client/v4/accounts//ai/v1").is_none());
        assert!(preset_of("https://api.cloudflare.com/client/v4/accounts/a/b/ai/v1").is_none());
        for (url, free) in [
            (cf, true),
            ("https://api.sambanova.ai/v1", true),
            ("https://integrate.api.nvidia.com/v1", true),
            ("https://api.groq.com/openai/v1", true),
            ("https://api.cerebras.ai/v1", false),
            ("https://api.deepseek.com/v1", false),
        ] {
            assert_eq!(is_free_endpoint(url, "x"), free, "{url}");
        }
        // GitHub Models se retiró.
        assert!(API_PRESETS.iter().all(|p| !p.base_url.contains("models.github.ai")));
        let ids: Vec<_> = API_PRESETS.iter().map(|p| p.id).collect();
        assert!(ids.iter().all(|id| ids.iter().filter(|o| *o == id).count() == 1), "{ids:?}");
        // Los gratis van primero.
        let first_paid = API_PRESETS.iter().position(|p| p.free.is_none()).unwrap();
        assert!(API_PRESETS[first_paid..].iter().all(|p| p.free.is_none()));
        // No se guarda una dirección que aún dice {cuenta}.
        let r = registry("cuenta");
        let err = save(&r, &Input {
            id: None,
            provider: "openai-compatible".into(),
            name: "Cloudflare".into(),
            config: json!({ "base_url": "https://api.cloudflare.com/client/v4/accounts/{cuenta}/ai/v1" }),
            secret: Some("x".into()),
        })
        .unwrap_err();
        assert!(format!("{err}").contains("ID de tu cuenta"), "{err}");
    }

    #[test]
    fn saves_connections_without_secret_and_validates_fields() {
        let r = registry("save");
        let err = save(&r, &Input {
            id: None,
            provider: "sqlite".into(),
            name: "Local".into(),
            config: json!({}),
            secret: None,
        })
        .unwrap_err();
        assert!(format!("{err}").contains("Archivo"));

        let c = save(&r, &Input {
            id: None,
            provider: "sqlite".into(),
            name: "Datos de la tienda".into(),
            config: json!({ "path": " /tmp/x.db ", "ignorado": "no" }),
            secret: None,
        })
        .unwrap();
        assert_eq!(c.id, "datos-de-la-tienda");
        assert_eq!(c.config, json!({ "path": "/tmp/x.db" }));
        assert!(c.secret_account.is_none());

        // Sin llave no se crea un Postgres.
        assert!(save(&r, &Input {
            id: None,
            provider: "postgres".into(),
            name: "pg".into(),
            config: json!({}),
            secret: None,
        })
        .is_err());
        // Una URL mal escrita se ataja antes de tocar el Llavero.
        assert!(validate_secret("postgres", "http://x").is_err());
        assert!(validate_secret("postgres", "postgres://u:p@h/db").is_ok());

        let servidor = save(&r, &Input {
            id: None,
            provider: "servidor".into(),
            name: "Casa".into(),
            config: json!({ "health_url": "miapp.com" }),
            secret: None,
        });
        assert!(servidor.is_err(), "la URL de salud debe llevar http(s)");
    }

    #[test]
    fn config_falls_back_to_field_defaults() {
        let c = Connection {
            id: "s".into(),
            kind: "hosting".into(),
            provider: "servidor".into(),
            name: "s".into(),
            config: json!({ "ssh_host": "h" }),
            secret_account: None,
            created_at: String::new(),
        };
        assert_eq!(c.config_str("ssh_user").as_deref(), Some("root"));
        assert_eq!(c.config_str("ssh_host").as_deref(), Some("h"));
        assert_eq!(c.config_str("health_url"), None);
    }

    #[test]
    fn links_respect_roles_and_kinds() {
        let r = registry("links");
        r.upsert(&crate::Project {
            slug: "tienda".into(),
            name: "Tienda".into(),
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
        let db = save(&r, &Input {
            id: None,
            provider: "sqlite".into(),
            name: "Datos".into(),
            config: json!({ "path": "/tmp/x.db" }),
            secret: None,
        })
        .unwrap();
        assert!(link(&r, "tienda", "hosting", &db.id, &json!({})).is_err());
        link(&r, "tienda", "base", &db.id, &json!({ "nota": 1 })).unwrap();
        link(&r, "tienda", "base", &db.id, &json!({ "nota": 2 })).unwrap();
        let found = linked(&r, "tienda", "base", Some("sqlite")).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.config["nota"], 2);
        assert!(has_link(&r, "tienda", "base", &db.id).unwrap());
        remove(&r, &db.id, false).unwrap();
        assert!(links(&r, "tienda").unwrap().is_empty(), "quitar la conexión quita sus vínculos");
    }
}
