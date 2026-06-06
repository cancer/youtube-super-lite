//! P1 移行実証バイナリ: 素の Win32 ウィンドウに mpv を D3D11 で埋め込んで再生する。
//!
//! inbox/opengl-to-native-migration.md の P1。本体アプリ(youtube-super-lite)は egui+OpenGL の
//! 自前合成だが、起動の瞬間に OpenGL ICD(nvoglv64.dll) がロードされて他アプリの GPU 動画が
//! カクつく。本 probe は **OpenGL コンテキストを一切作らず**、mpv 自身の `vo=gpu-next`
//! `gpu-api=d3d11` に HWND(`wid`) を渡して描画させることで、起動時の GPU 競合が解消するかを
//! 検証する。ブラウザで YouTube を再生しながら本 probe を起動し、カクつきの有無を比較する。
//!
//! 使い方:
//!   cargo run --bin mpv_d3d11_probe                 # 映像なしでも D3D11 vo を即時生成 (force-window)
//!   cargo run --bin mpv_d3d11_probe -- <file|url>   # 指定メディアを再生
//!   cargo run --bin mpv_d3d11_probe -- av://lavfi:testsrc2=size=1280x720:rate=30  # テストパターン

#[cfg(not(windows))]
fn main() {
    eprintln!("mpv_d3d11_probe は Windows 専用です (D3D11 埋め込みの実証)。");
}

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    use anyhow::anyhow;
    use libmpv2::Mpv;
    use windows::core::w;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, LoadCursorW,
        PostQuitMessage, RegisterClassW, TranslateMessage, CW_USEDEFAULT, IDC_ARROW, MSG,
        WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
    };

    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        use windows::Win32::UI::WindowsAndMessaging::WM_DESTROY;
        match msg {
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let class_name = w!("MpvD3D11ProbeClass");

        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(anyhow!("RegisterClassW failed"));
        }

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("mpv D3D11 probe (no OpenGL)"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            720,
            None,
            None,
            hinstance,
            None,
        )?;

        // HWND を mpv の `wid` に渡して埋め込む。HWND.0 は *mut c_void。
        let wid: i64 = hwnd.0 as isize as i64;
        eprintln!("[probe] HWND(wid)={wid}");

        // mpv を D3D11 出力で初期化。OpenGL/Render API は一切使わない。
        let mpv = Mpv::with_initializer(|init| {
            init.set_property("wid", wid)?;
            init.set_property("vo", "gpu-next")?;
            init.set_property("gpu-api", "d3d11")?;
            init.set_property("hwdec", "auto")?;
            // YouTube 解決は本体アプリ側の責務。probe では直リンク/ローカルのみ。
            init.set_property("ytdl", false)?;
            // メディアが無くても起動時に D3D11 vo を生成させる（GPU bring-up を即時に起こす）。
            init.set_property("force-window", "yes")?;
            init.set_property("idle", "yes")?;
            // どの GPU API/デコーダが選ばれたかを確認する。stderr は親プロセスの
            // リダイレクトでバッファされ得るので、mpv 自身のログファイルにも書き出す。
            init.set_property("terminal", true)?;
            init.set_property("msg-level", "all=v")?;
            init.set_property("log-file", "probe-mpv.log")?;
            Ok(())
        })
        .map_err(|e| anyhow!("mpv init failed: {e}"))?;

        if let Some(path) = std::env::args().nth(1) {
            eprintln!("[probe] loadfile {path}");
            mpv.command("loadfile", &[&path])
                .map_err(|e| anyhow!("loadfile failed: {e}"))?;
        } else {
            eprintln!("[probe] メディア未指定: force-window で D3D11 vo のみ生成（黒画面）");
        }

        // メッセージループ。mpv は wid に対し自前スレッドで D3D11 描画する。
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    Ok(())
}
