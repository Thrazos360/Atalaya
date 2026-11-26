# Puerta de administración (protocolo 1)

Cada app que se administra desde Atalaya expone una sola ruta, `/api/atalaya`. Atalaya
nunca escribe en la base de una app: le pide sus controles y le manda un cambio a la vez.
La app valida y aplica cada cambio con su propia lógica y lo deja en su propia auditoría.

Una implementación típica en Next.js vive en `src/lib/atalaya/` (controles y validación)
y `src/app/api/atalaya/route.ts` (la ruta).

## Seguridad

- La protege una llave de 64 caracteres hexadecimales en la variable `ATALAYA_ADMIN_KEY`.
  La genera `atalaya admin connect <slug> <dirección>`, que la guarda en el Llavero
  (cuenta `admin-<slug>`) y, si la app está en Vercel, como variable de producción.
- Sin esa variable la ruta responde 404: una instalación que no usa Atalaya no expone nada.
- Llave ausente o distinta: 401. Comparar con hashes de largo fijo (`timingSafeEqual`).
- Siempre por https (Atalaya rechaza http salvo `localhost`).
- La ruta queda fuera del middleware o proxy de sesión de la app.

## `GET /api/atalaya`

```json
{
  "protocol": 1,
  "app": "MiApp",
  "sections": [
    {
      "id": "clientes",
      "title": "Clientes",
      "description": "Texto para la persona",
      "items": [
        {
          "id": "uuid-del-cliente",
          "title": "Acme",
          "detail": "acme · Empresa · Activo",
          "tone": "ok | aviso | falla",
          "controls": [
            { "key": "plan", "label": "Plan", "type": "select", "value": "empresa",
              "options": [{ "value": "inicio", "label": "Inicio" }], "group": "Cuenta" },
            { "key": "estado", "label": "Estado", "type": "select", "danger": true, "value": "activo",
              "options": [], "help": "Suspender corta el acceso." }
          ]
        }
      ],
      "create": { "label": "Publicar anuncio", "controls": [] }
    }
  ]
}
```

Tipos de control: `switch` (booleano), `select` (una de `options`), `number` (entero, con
`min`/`max`), `text` y `textarea` (con `maxLength`), `date` (`AAAA-MM-DD`) y `action` (un
botón; su valor se ignora). `optional: true` permite dejarlo vacío (`null`). `danger: true`
hace que Atalaya pida confirmación antes de mandarlo. `group` agrupa en pantalla.

## `POST /api/atalaya`

Un cambio:

```json
{ "section": "clientes", "item": "uuid-del-cliente", "key": "feature:facturas", "value": "on" }
```

Un elemento nuevo, con los campos de `section.create`:

```json
{ "section": "anuncios", "create": { "titulo": "Mantención el sábado", "dias": "1" } }
```

Respuesta: `{ "ok": true, "message": "Texto para la persona" }` con 200, o
`{ "ok": false, "error": "Por qué no" }` con 400. La app solo acepta controles que ofrece en
ese momento su `GET` y valida el valor contra lo que declara.

## Desde Atalaya

- Ventana: panel **Administración** en la página de cada proyecto.
- Terminal: `atalaya admin connect | show | set | key`.
- MCP: `admin_controls` y `admin_set`.
