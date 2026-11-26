import { invoke } from "@tauri-apps/api/core";

export type Status = "ok" | "aviso" | "falla";

export type Project = {
  slug: string;
  name: string;
  description: string | null;
  local_path: string | null;
  repo: string | null;
  production_url: string | null;
};

export type Check = {
  slug: string;
  kind: string;
  status: Status;
  summary: string;
  details: Record<string, unknown> | null;
  checked_at: string;
  acked: boolean;
};

export type Live = {
  error?: string;
  git: { branch: string; uncommitted_files: number; recent_commits: string[] } | null;
  stack: {
    scripts: string[];
    dependencies: Record<string, string>;
    dev_dependencies: Record<string, string>;
  } | null;
  services: {
    vercel_project_id: string | null;
    supabase_project_ref: string | null;
    firebase_project: string | null;
  };
  docs: { file: string; content: string; truncated: boolean }[];
};

export type Context = { project: Project; live: Live | null; checks: Check[] };

export type Flags = Record<string, { on: boolean; desc: string; value?: string }>;

export type ImprovementState = "propuesta" | "aceptada" | "en_curso" | "en_revision" | "hecha" | "descartada";

export type Improvement = {
  id: number;
  slug: string;
  title: string;
  detail: string;
  status: ImprovementState;
  source: string;
  created_at: string;
  updated_at: string;
  branch: string | null;
  pr_url: string | null;
  result: string | null;
  feedback: string | null;
};

export type Repo = {
  full_name: string;
  name: string;
  description: string | null;
  private: boolean;
  homepage: string | null;
  pushed_at: string | null;
  default_branch: string;
  url: string;
};

/** Un día de la historia del sitio. */
export type DayStat = { day: string; total: number; ok: number; ms: number | null };

export type SystemStatus = {
  version: string;
  database: string;
  service_running: boolean | null;
  keys: { service: string; saved: boolean }[];
  rounds: { kind: string; every_seconds: number }[];
  email: { address: string | null; to: string | null; report_hour: number; ready: boolean };
  flags_store: string | null;
  auto_merge: boolean;
};

export type ProviderField = { key: string; label: string; placeholder: string; required: boolean; default: string; hidden: boolean };

export type Provider = {
  id: string;
  kind: string;
  label: string;
  description: string;
  fields: ProviderField[];
  secret: string | null;
  help_url: string;
  single: boolean;
};

export type TestResult = { ok: boolean; message: string; at: string };

export type ConnectionItem = {
  id: string;
  kind: string;
  provider: string;
  name: string;
  config: Record<string, string>;
  has_secret: boolean;
  legacy: boolean;
  last_test: TestResult | null;
  used_by: string[];
  created_at: string;
};

export type Connections = {
  connections: ConnectionItem[];
  providers: Provider[];
  kinds: { id: string; label: string }[];
  roles: { id: string; label: string }[];
};

export type ConnectionInput = {
  id?: string;
  provider: string;
  name: string;
  config: Record<string, string>;
  secret?: string;
};

export type Link = { slug: string; role: string; connection_id: string; config: Record<string, string> };

export const api = {
  overview: () => invoke<Context[]>("overview"),
  runChecks: (slug: string) => invoke<void>("run_checks", { slug }),
  ackAlert: (slug: string, kind: string, acked: boolean) => invoke<void>("ack_alert", { slug, kind, acked }),
  resolveAlert: (slug: string, kind: string) => invoke<number>("resolve_alert", { slug, kind }),
  setAutoMerge: (on: boolean) => invoke<void>("set_auto_merge", { on }),
  flags: (command: string, slug: string, args: Record<string, unknown> = {}) =>
    invoke<Flags>(command, { slug, ...args }),
  improvements: () => invoke<Improvement[]>("list_improvements"),
  addImprovement: (slug: string, title: string, detail: string) =>
    invoke<void>("add_improvement", { slug, title, detail }),
  setImprovementStatus: (id: number, status: ImprovementState) =>
    invoke<void>("set_improvement_status", { id, status }),
  deleteImprovement: (id: number) => invoke<void>("delete_improvement", { id }),
  startImprovement: (id: number) => invoke<void>("start_improvement", { id }),
  answerImprovement: (id: number, answer: string) => invoke<Improvement>("answer_improvement", { id, answer }),
  queuedImprovements: () => invoke<number[]>("queued_improvements"),
  mergeImprovement: (id: number) => invoke<Improvement>("merge_improvement", { id }),
  systemStatus: () => invoke<SystemStatus>("system_status"),
  takeNotice: () => invoke<string | null>("take_notice"),
  siteHistory: (slug: string) => invoke<DayStat[]>("site_history", { slug }),
  cloneProject: (slug: string) => invoke<string>("clone_project", { slug }),
  backupProject: (slug: string) => invoke<void>("backup_project", { slug }),
  githubRepos: () => invoke<Repo[]>("github_repos"),
  githubCreate: (name: string, desc: string, isPrivate: boolean) =>
    invoke<Repo>("github_create", { name, desc, private: isPrivate }),
  githubWatch: (repo: string, url: string | null) => invoke<void>("github_watch", { repo, url }),
  removeProject: (slug: string) => invoke<void>("remove_project", { slug }),
  connections: () => invoke<Connections>("connections_list"),
  saveConnection: (input: ConnectionInput) => invoke<{ id: string; name: string }>("connections_save", { input }),
  testConnection: (id: string) => invoke<string>("connections_test", { id }),
  removeConnection: (id: string, forgetSecret: boolean) => invoke<void>("connections_remove", { id, forgetSecret }),
  links: (slug: string) => invoke<Link[]>("links_get", { slug }),
  setLink: (slug: string, role: string, connectionId: string, config: Record<string, string>) =>
    invoke<void>("links_set", { slug, role, connectionId, config }),
  unlink: (slug: string, role: string, connectionId: string) => invoke<boolean>("links_unlink", { slug, role, connectionId }),
};

export const KIND_LABEL: Record<string, string> = {
  sitio: "Sitio",
  certificado: "Certificado",
  migraciones: "Migraciones",
  respaldo: "Respaldo",
  github: "GitHub",
  firebase: "Firebase",
  despliegue: "Despliegue",
  supabase: "Supabase",
  git: "Git",
  dependencias: "Dependencias",
  base: "Base de datos",
  servidor: "Servidor",
  repositorio: "Repositorio",
};

/** El peor estado entre las revisiones de un proyecto. */
export function worst(checks: Check[]): Status {
  if (checks.some((c) => c.status === "falla")) return "falla";
  return checks.some((c) => c.status === "aviso") ? "aviso" : "ok";
}

export type Alert = Check & { project: Project };

/** Toda revisión que no está en orden es una alerta; las fallas van primero. */
export function alertsOf(contexts: Context[]): Alert[] {
  return contexts
    .flatMap(({ project, checks }) => checks.filter((c) => c.status !== "ok").map((c) => ({ ...c, project })))
    .sort((a, b) => Number(b.status === "falla") - Number(a.status === "falla") || b.checked_at.localeCompare(a.checked_at));
}

const asDate = (utc: string) => new Date(utc.replace(" ", "T") + "Z");

/** Las fechas se guardan en UTC ("2026-11-23 01:39:42"). */
export function localTime(utc: string) {
  return asDate(utc).toLocaleString("es", { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
}

export function ago(utc: string) {
  const minutes = Math.max(0, Math.round((Date.now() - asDate(utc).getTime()) / 60000));
  if (minutes < 1) return "ahora";
  if (minutes < 60) return `hace ${minutes} min`;
  const hours = Math.round(minutes / 60);
  return hours < 24 ? `hace ${hours} h` : `hace ${Math.round(hours / 24)} d`;
}

/** "46f36ee 2026-11-22 Mensaje" → partes. */
export function parseCommit(line: string) {
  const [hash, date, ...rest] = line.split(" ");
  return { hash, date, message: rest.join(" ") };
}

export function everyLabel(seconds: number) {
  return seconds >= 3600 ? `cada ${seconds / 3600} h` : `cada ${seconds / 60} min`;
}

/** Datos simples del detalle de una revisión, como "codigo: 404 · ms: 210". */
export function detailLine(details: Check["details"]) {
  return Object.entries(details ?? {})
    .map(([key, value]) => {
      if (value === null || value === "") return null;
      if (Array.isArray(value)) return value.length ? `${key}: ${value.length}` : null;
      if (typeof value === "object") return null;
      return `${key}: ${value}`;
    })
    .filter(Boolean)
    .join(" · ");
}
