# Cómo colaborar con Atalaya

Gracias por darte el tiempo. Atalaya está en desarrollo activo, y todo aporte ayuda: un
reporte de error, una idea, un proveedor nuevo o una mejora de la interfaz.

## Antes de empezar

- **¿Encontraste un error?** Abre un [issue](https://github.com/Thrazos360/Atalaya/issues).
  Cuenta qué hiciste, qué esperabas y qué pasó, y agrega la versión, que aparece en la barra
  lateral de la app y en `atalaya --version`. Antes de pegar registros, revisa que no tengan
  llaves, rutas ni nombres de tus proyectos.
- **¿Una idea grande?** Ábrela primero como issue y la conversamos antes de que escribas
  código.
- **¿Un problema de seguridad?** No lo publiques como issue. Usa
  [Report a vulnerability](https://github.com/Thrazos360/Atalaya/security/advisories/new),
  que llega en privado.

## Preparar el entorno

Necesitas macOS, Rust estable, Node 20 o más nuevo y las herramientas de línea de comandos de
Xcode.

```bash
cargo test --workspace
```

```bash
cd app && npm install && npm run build
```

Para probar sin tocar tu instalación, usa una base y un Llavero aparte:

```bash
ATALAYA_DB=/tmp/atalaya-prueba.db ATALAYA_KEYCHAIN_SERVICE=Atalaya-prueba cargo run -p atalaya -- doctor
```

Cómo está armado el código, y la receta para agregar un proveedor o una IA, está en
[docs/ARQUITECTURA.md](docs/ARQUITECTURA.md).

## Reglas de la casa

- **Pruebas en verde.** Cada pieza nueva lleva su prueba, sin depender de la red: usa los
  servidores falsos de `testutil.rs` y de las pruebas de `ai.rs`.
- **Las migraciones solo agregan.** Nunca borres tablas, columnas ni datos de la base de
  quien usa Atalaya.
- **Nada de secretos.** Las llaves van al Llavero, nunca a la base, a los registros ni al
  código. Antes de subir, corre `cargo run -p atalaya -- secretos .`.
- **Nada sin visto bueno.** Una función que cambia algo fuera de Atalaya (un repo, un
  despliegue, una base) pasa por una decisión o una aprobación.
- **Textos en español, simples y directos.** La interfaz le habla a la persona de tú, sin
  jerga cuando no hace falta.
- **Formato.** `cargo fmt` para Rust, y en los `.tsx` que toques,
  `npx prettier --print-width 130 --write <archivo>`.

## El pull request

1. Crea una rama desde `main`.
2. Haz commits chicos, con mensajes que digan qué cambia y por qué.
3. Si el cambio se nota, agrega una línea en `CHANGELOG.md`, en la versión que viene.
4. Abre el pull request contando qué resuelve y cómo lo probaste.

Al colaborar, aceptas que tu aporte se publique con la misma licencia del proyecto,
[MIT](LICENSE).
