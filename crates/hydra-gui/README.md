# hya-gui

Desktop GUI for hydra, built with [iced 0.14](https://iced.rs) as a
multi-window daemon: the main window and every dialog (Add URL, Download File
Info, download progress, Configuration, Scheduler, batch add, confirmations)
is its own OS window.

```
cargo run -p hya-gui
```

## Architecture

| module | role |
|---|---|
| `main.rs` | `iced::daemon` glue: boot, per-window view/title dispatch, subscriptions (engine events, native-menu events, 1 s scheduler tick, window close) |
| `app.rs` | the one shared `App` state + `Message` enum + all update logic |
| `engine.rs` | bridge to `hya-core`/`hya-net`: a dedicated tokio runtime thread owning every transfer; commands in, progress events out |
| `model.rs` | persisted data model + `config.toml` / `gui-state.json` I/O |
| `theme.rs` / `icons.rs` | Visual language: light/dark styles, gradient-outline SVG toolbar icons |
| `ui/` | main-window widgets: menu model + in-window menu bar, toolbar, categories tree, download table |
| `windows/` | one view per window kind |
| `i18n.rs` | gettext-style translation (`tr("English text")`), JSON catalogues |
| `macos_menu.rs` | native macOS menu bar via `muda` (Windows/Linux draw the in-window bar) |

## Engine

Range-capable origins are downloaded with the `hya-core` scheduler over N
connections (default 8; per-server exceptions in Options >
Connection). Pause/Cancel go through `hya_net::run_transfer_cancellable`'s
stop flag so sockets actually close; received byte spans are persisted and
re-`mark_done`d on resume — including across app restarts. The Speed Limiter
(global and per-download) drives one shared `RateLimiter`, changeable live.
Servers without range support fall back to a single streaming GET.

## Files on disk

- Linux/macOS: `~/.config/hydra/` — Windows: `%APPDATA%\hydra\`
  - `config.toml` — settings, categories, queues (unparsable file ⇒ defaults)
  - `gui-state.json` — the download list
  - `locales/<tag>.json` — translation catalogues, e.g. `fa.json`:
    `{ "Add URL": "افزودن پیوند" }` (msgid = the English string; missing keys
    fall back to English)
  - `logs/gui.log` — session log
  - `parts/` — default temporary directory for in-flight `.part` files

## Testing

`cargo test -p hya-gui` runs the unit tests. Set
`HYDRA_GUI_LIVE_TEST=<url>` to also exercise the full live engine path
(probe → multi-connection transfer → rename) against a real origin.
