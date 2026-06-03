//! GPU 使用率の取得と、それに基づく mpv のハードウェアデコード自動フォールバック。
//!
//! Windows 限定。`--auto-hwdec-fallback` フラグが立っているとき、別スレッドで
//! Performance Data Helper (PDH) の `\GPU Engine(*)\Utilization Percentage`
//! カウンタを 1 秒間隔でポーリングし、外部アプリ含めた全体 GPU 負荷が高ければ
//! mpv の `hwdec` を `no` に倒し、低くなれば `auto` に戻す。
//!
//! 動画描画 (OpenGL) は GPU を引き続き使うが、HW デコード分の GPU 負荷は
//! 軽くなる（4K/60fps なら 10-30% 程度）。ゲーム・エンコーダ等の外部アプリに
//! GPU を譲るのが主な目的。
//!
//! macOS 等では NOP。

#[cfg(windows)]
mod imp {
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    };

    /// SW へ切替を要求する閾値（%）と、その状態を維持する継続時間。
    const SWITCH_TO_SW_THRESHOLD: f64 = 80.0;
    const SWITCH_TO_SW_HOLD: Duration = Duration::from_secs(3);
    /// HW へ復帰を要求する閾値（%）と、その状態を維持する継続時間。
    const SWITCH_TO_HW_THRESHOLD: f64 = 60.0;
    const SWITCH_TO_HW_HOLD: Duration = Duration::from_secs(5);

    /// 監視ループからメインスレッドへの通知。
    #[derive(Debug, Clone, Copy)]
    pub enum HwdecDecision {
        UseSoftware,
        UseHardware,
    }

    /// GPU 使用率の監視を開始する。停止するには [`Monitor::stop`] を呼ぶ。
    pub fn start_monitoring() -> Option<Monitor> {
        let (tx, rx) = mpsc::channel();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let handle = thread::Builder::new()
            .name("gpu-usage-monitor".into())
            .spawn(move || run_loop(tx, stop_rx))
            .ok()?;
        Some(Monitor {
            rx,
            stop_tx,
            handle: Some(handle),
        })
    }

    pub struct Monitor {
        rx: mpsc::Receiver<HwdecDecision>,
        stop_tx: mpsc::Sender<()>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl Monitor {
        /// 直近の決定を取り出す（非ブロッキング）。
        pub fn try_recv(&self) -> Option<HwdecDecision> {
            self.rx.try_recv().ok()
        }

        /// 監視を停止して結合する。
        pub fn stop(mut self) {
            let _ = self.stop_tx.send(());
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }

    fn run_loop(tx: mpsc::Sender<HwdecDecision>, stop_rx: mpsc::Receiver<()>) {
        let mut query: PDH_HQUERY = Default::default();
        let mut counter: PDH_HCOUNTER = Default::default();

        // SAFETY: PDH API はハンドルアウトパラメータ経由。失敗時は NULL ハンドルにならず
        // ERROR_SUCCESS 以外を返す。失敗時は監視そのものを諦める。
        unsafe {
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != ERROR_SUCCESS.0 {
                eprintln!("[gpu-usage] PdhOpenQueryW 失敗");
                return;
            }
            let path: Vec<u16> = "\\GPU Engine(*)\\Utilization Percentage\0"
                .encode_utf16()
                .collect();
            if PdhAddEnglishCounterW(query, PCWSTR(path.as_ptr()), 0, &mut counter)
                != ERROR_SUCCESS.0
            {
                eprintln!("[gpu-usage] PdhAddEnglishCounterW 失敗");
                let _ = PdhCloseQuery(query);
                return;
            }
            // 初回 collect は値を埋めるためのウォームアップ。
            let _ = PdhCollectQueryData(query);
        }

        let mut last_decision: Option<HwdecDecision> = None;
        let mut high_since: Option<Instant> = None;
        let mut low_since: Option<Instant> = None;

        loop {
            // 停止シグナル。ポーリング間隔分だけ待つ。
            match stop_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }

            let usage = unsafe { sample(query, counter) };
            let Some(usage) = usage else { continue };

            let now = Instant::now();
            if usage >= SWITCH_TO_SW_THRESHOLD {
                low_since = None;
                let since = *high_since.get_or_insert(now);
                if now.duration_since(since) >= SWITCH_TO_SW_HOLD
                    && !matches!(last_decision, Some(HwdecDecision::UseSoftware))
                {
                    last_decision = Some(HwdecDecision::UseSoftware);
                    let _ = tx.send(HwdecDecision::UseSoftware);
                }
            } else if usage <= SWITCH_TO_HW_THRESHOLD {
                high_since = None;
                let since = *low_since.get_or_insert(now);
                if now.duration_since(since) >= SWITCH_TO_HW_HOLD
                    && !matches!(last_decision, Some(HwdecDecision::UseHardware))
                {
                    last_decision = Some(HwdecDecision::UseHardware);
                    let _ = tx.send(HwdecDecision::UseHardware);
                }
            } else {
                // 中間帯（60〜80）はヒステリシスとして現状維持。タイマーはリセットしない。
            }
        }

        unsafe {
            let _ = PdhCloseQuery(query);
        }
    }

    /// `\GPU Engine(*)\Utilization Percentage` の全インスタンスを集めて、
    /// **engine_type ごとに合計したうちの最大値** を「GPU 全体使用率」として返す。
    /// 単一 instance の合計だと 100% を超えるので適切に丸めない。
    unsafe fn sample(query: PDH_HQUERY, counter: PDH_HCOUNTER) -> Option<f64> {
        if PdhCollectQueryData(query) != ERROR_SUCCESS.0 {
            return None;
        }
        let mut buffer_size: u32 = 0;
        let mut item_count: u32 = 0;
        // 必要バッファサイズ取得 (1 回目)。
        let _ = PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut buffer_size,
            &mut item_count,
            None,
        );
        if buffer_size == 0 || item_count == 0 {
            return None;
        }
        let mut buf = vec![0u8; buffer_size as usize];
        let items_ptr = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE,
            &mut buffer_size,
            &mut item_count,
            Some(items_ptr),
        ) != ERROR_SUCCESS.0
        {
            return None;
        }
        let items = std::slice::from_raw_parts(items_ptr, item_count as usize);

        // インスタンス名は `pid_PID_luid_X_Y_phys_P_eng_E_engtype_TYPE`。
        // engine_type ごとに合計し、最大値を採用する。
        let mut by_engtype: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        for it in items {
            let raw = read_wide_to_string(it.szName.0);
            let engtype = raw
                .rsplit_once("_engtype_")
                .map(|(_, t)| t.to_string())
                .unwrap_or_else(|| "_unknown".to_string());
            let v = it.FmtValue.Anonymous.doubleValue;
            *by_engtype.entry(engtype).or_insert(0.0) += v;
        }
        by_engtype.values().cloned().fold(None, |acc, v| {
            Some(acc.map(|a: f64| a.max(v)).unwrap_or(v))
        })
    }

    unsafe fn read_wide_to_string(p: *const u16) -> String {
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(p, len);
        String::from_utf16_lossy(slice)
    }
}

#[cfg(not(windows))]
mod imp {
    /// 非 Windows では監視を行わないので [`start_monitoring`] が常に [`None`] を返し、
    /// 結果として `HwdecDecision` の値は生成されない。ただし呼び出し側 (main.rs) で
    /// match パターンが書けるよう、バリアントだけ宣言しておく。
    pub enum HwdecDecision {
        UseSoftware,
        UseHardware,
    }

    pub struct Monitor;
    impl Monitor {
        pub fn try_recv(&self) -> Option<HwdecDecision> {
            None
        }
        pub fn stop(self) {}
    }

    pub fn start_monitoring() -> Option<Monitor> {
        None
    }
}

pub use imp::{start_monitoring, HwdecDecision, Monitor};
