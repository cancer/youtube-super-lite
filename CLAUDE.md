# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Talava Player is a Windows-only Rust YouTube player: libmpv renders video into an OpenGL framebuffer and egui draws the UI on top. A Cloudflare Worker (`auth-worker/`) holds the OAuth client secret so the desktop binary never ships it.

## Building / running

`cargo build` **does not work on its own.** Linking libmpv needs the MSVC environment and a link-search path that the crates don't set. Always build via:

```powershell
.\build.ps1            # debug
.\build.ps1 -Release   # release
```

`build.ps1` loads `vcvars64.bat`, sets `MPV_SOURCE=tools\mpv-dev`, runs `cargo build`, then copies `libmpv-2.dll` and `yt-dlp.exe` next to the exe (both are required at runtime and are git-ignored). If you must call cargo directly, replicate that environment.

Run:
```powershell
.\target\debug\talava-player.exe "https://www.youtube.com/watch?v=..."
$env:TALAVA_VERBOSE = "1"   # mpv status line + frame counter on stderr
$env:TALAVA_AUTH_BACKEND = "https://<worker>.workers.dev"   # default http://127.0.0.1:8787
```

There is **no test suite**. Verification is done by launching the app (window appears) and grepping stderr for `AV:` / `[frames]` progression with `TALAVA_VERBOSE=1`.

### Toolchain prerequisites
- Rust **MSVC** toolchain + Visual Studio Build Tools 2022 (VC Tools).
- `tools/mpv-dev/` (libmpv dev package from shinchiro/mpv-winbuild-cmake) and `tools/yt-dlp.exe`. `libmpv-2.dll` and `yt-dlp.exe` are git-ignored — re-fetch them when cloning fresh (see README).
- For `auth-worker/`: Node 18+, `wrangler`, the `wasm32-unknown-unknown` target. PowerShell's execution policy blocks `npx.ps1`; invoke wrangler through `cmd /c "... npx wrangler ..."`.

## libmpv linking (the fragile part)

- The crate `libmpv2-sys` only emits `cargo:rustc-link-lib=mpv`; it does **not** add a search path. [build.rs](build.rs) adds `tools/mpv-dev` so the linker finds `mpv.lib`.
- `tools/mpv-dev/mpv.lib` is an MSVC import library generated from the DLL's exports (`dumpbin /exports libmpv-2.dll` → `mpv.def` → `lib /def:... /name:libmpv-2.dll`). Regenerate it (steps in README) if the DLL changes.
- **`libmpv2` must stay on 6.x.** 4.x crashes (`0xC0000005`) at `RenderContext` creation due to a use-after-free in the Render API path.

## Rendering architecture ([src/main.rs](src/main.rs))

- winit 0.30 `ApplicationHandler`. All live state lives in `Running`, created in `App::init` on `resumed`.
- glutin creates the GL context; its `get_proc_address` feeds **both** glow (for egui_glow) and the mpv `RenderContext`. mpv uses `vo=libmpv` (not `gpu`/`wid`).
- Each `redraw`: make context current → `render_context.render(fbo=0, ...)` draws the video into the default framebuffer → egui draws the UI on top → `swap_buffers`. Video is the bottom layer; egui composites over it.
- **`Mpv` is `Box::leak`'d to `'static`** so `RenderContext<'static>` (which borrows `Mpv`) can be stored in the same struct without a self-referential type.
- Redraws are driven by mpv's update callback → `EventLoopProxy<UserEvent>` (`MpvRedraw`) → `request_redraw`. Background/auth threads wake the loop with `UserEvent::Background`. `ControlFlow::Wait` otherwise.
- UI auto-hide: panels are skipped (and the cursor hidden) after `UI_HIDE_AFTER` of no mouse/key activity. Because mpv doesn't drive redraws while paused, `redraw` self-requests the next frame when `paused && show_ui` so the hide countdown still advances; vsync caps that loop.
- yt-dlp is found via `ensure_ytdlp_on_path` (prepends the bundled dir to PATH); mpv's ytdl_hook then resolves YouTube URLs.

## Auth architecture ([src/auth.rs](src/auth.rs) + [auth-worker/](auth-worker/))

The desktop app **never holds `client_secret`.** Only the browser consent step is delegated to the browser; everything else is in-app API calls.

- Login: `auth::login` runs a loopback OAuth code flow (`TcpListener` on `127.0.0.1:<port>`, opens the consent URL via `crate::open_in_browser`, captures the redirect). The authorization-code/refresh exchange is POSTed to the **Cloudflare Worker** (`/token`, `/refresh`), which appends `client_id`/`client_secret` and relays to Google. The app only knows the Worker URL (`TALAVA_AUTH_BACKEND`).
- API calls that need only an access token (`videos.rate` for like, `channels?mine=true` for the name) are made directly from the app.
- Refresh token persists at `%APPDATA%\TalavaPlayer\auth.json`; the app auto-logs-in on startup if present.
- All network runs on spawned threads; results return via `mpsc` (`AuthMsg`) and are drained in `redraw`/`poll_auth`. **egui closures only set intent flags** (`login_clicked`, `like_clicked`, `to_load`); the actual `start_login`/`start_like`/`load` run *after* `egui_glow.run` returns, because `egui_glow` is borrowed mutably during the closure. Follow this pattern when adding UI actions that touch `self`.

### The Worker ([auth-worker/src/lib.rs](auth-worker/src/lib.rs), workers-rs)
- Endpoints: `GET /client_id`, `POST /token`, `POST /refresh`. Reads var `GAUTH_CLIENT_ID` and secret `GAUTH_CLIENT_SECRET`.
- Deploy from `auth-worker/`: set `GAUTH_CLIENT_ID` in `wrangler.jsonc`, `wrangler secret put GAUTH_CLIENT_SECRET`, then `wrangler deploy` (the `build` command compiles Rust→WASM via `worker-build`).
- The Google OAuth client must be type **Desktop app** (loopback `127.0.0.1` redirect) with scope `youtube.force-ssl`, and the user must be a test user on the consent screen.
