# Claude Code Usage Monitor + insignia de actividad

Es [Claude Code Usage Monitor](https://github.com/CodeZeno/Claude-Code-Usage-Monitor) (CodeZeno, MIT) **tal cual**, con un añadido: una insignia animada al lado del widget de 5h / 7d en la barra de tareas que te dice qué está haciendo Claude cuando no estás mirando su ventana.

| Estado | Insignia |
|---|---|
| Claude trabajando | animación naranja a elegir: puntos, anillo, barras, pulso o destello |
| Necesita tu permiso / una respuesta | círculo ámbar con "!" y pulso |
| El turno falló (error de API) | círculo rojo con "!" |
| Terminó | estallido → check verde; queda recordándote hasta que vuelvas |

- Solo aparece si la ventana de esa sesión (VS Code, la app de Claude, una terminal) **no** está al frente.
- **Clic izquierdo**: te lleva a esa ventana. **Clic derecho**: la oculta (en "trabajando", hasta el próximo prompt).
- Funciona con Claude Code en VS Code, en la terminal y en la app de escritorio. El chat normal de la app de Claude no tiene hooks, así que no se detecta.

## Cómo se mantiene al día

Este repo **no** es una copia del código de CodeZeno: solo guarda el añadido (`overlay/`) y un script que lo aplica (`scripts/apply.py`, toca 3 líneas de `main.rs`).
La GitHub Action `build.yml` revisa cada 6 horas si CodeZeno publicó un release nuevo; si sí, descarga ese release, aplica el añadido, corre las pruebas, compila y publica aquí el `.exe`.
La app trae su propio actualizador y apunta a los releases **de este repo**, así que se actualiza sola.

Versión: upstream `X.Y.Z` → aquí `X.Y.(Z×100 + REVISION)`. Ej.: upstream 2.23.0 → 2.23.1. Cuando cambies el añadido, sube el número de `overlay/REVISION` para publicar una versión nueva.

Si un cambio de upstream rompe el añadido, la Action falla y abre un issue en este repo.

## Instalar

1. Cierra el monitor actual y, si lo instalaste con winget, desinstálalo: `winget uninstall CodeZeno.ClaudeCodeUsageMonitor` (si no, winget lo vuelve a la versión original).
2. Descarga `claude-code-usage-monitor.exe` de [Releases](../../releases) a una carpeta fija, p. ej. `C:\Utilidades\ClaudeMonitor\`.
3. Ábrelo. A los pocos segundos registra sus hooks en `%USERPROFILE%\.claude\settings.json` (deja copia en `settings.json.ccum-activity.bak` y no toca tus otras opciones).
4. Reinicia las sesiones de Claude Code abiertas (VS Code: *Developer: Reload Window*; app de Claude: ciérrala y ábrela).
5. Activa *Start with Windows* desde el menú del monitor para que arranque esta versión.

Para ver las animaciones sin usar Claude: `claude-code-usage-monitor.exe --activity-demo` (con el monitor abierto, minimiza todo y mira la barra durante ~25 s).

## Opciones

`%LOCALAPPDATA%\ClaudeCodeUsageMonitor-Activity\settings.json` (se crea solo; se relee al guardar):

```json
{
  "enabled": true,
  "style": "bars",
  "show_when_focused": false,
  "position": "auto",
  "size_percent": 100,
  "auto_install_hooks": true,
  "done_minutes": 30,
  "stale_minutes": 20
}
```

- `style`: animación de "trabajando": `bars` (barras, por defecto), `dots` (puntos), `ring` (anillo), `pulse` (pulso) o `spark` (destello).
- `position`: `auto` (el lado con más espacio), `left` o `right`.
- `show_when_focused`: `true` para verla aunque tengas la ventana de Claude al frente.
- `done_minutes`: cuánto tiempo sigue recordándote que terminó.
- `stale_minutes`: si una sesión "trabajando" no da señales en ese tiempo, se oculta.

Quitar los hooks: `claude-code-usage-monitor.exe --activity-uninstall-hooks`. Volver a ponerlos: `--activity-install-hooks`.

## Cómo funciona

Claude Code llama a `claude-code-usage-monitor.exe --activity-hook` (en segundo plano, `async`) en `UserPromptSubmit`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`, `Notification`, `Stop`, `StopFailure` y `SessionEnd`. Ese proceso escribe un JSON pequeño por sesión en `%LOCALAPPDATA%\ClaudeCodeUsageMonitor-Activity\sessions\` y termina; nunca imprime nada ni bloquea a Claude.
Al enviar un prompt también anota qué ventana lo recibió, para saber luego si la tienes al frente.
El monitor lee esos archivos cada 250 ms y dibuja la insignia en una ventana propia anclada al widget. Si cancelas un turno con Esc, se detecta leyendo el final de la transcripción.

## Desarrollo

```bash
git clone https://github.com/CodeZeno/Claude-Code-Usage-Monitor upstream
python scripts/apply.py upstream https://github.com/<tu-usuario>/<este-repo>
cd upstream && cargo test activity && cargo build --release
```

Licencia MIT, igual que el proyecto original.
