//! nsig（n パラメータ）変換。認証経路(TVHTML5)の stream URL は `n` が未変換で 403 になるため、
//! base.js を JS エンジンにロードして変換する。
//!
//! 設計（交換可能性）: JS エンジンは狭いトレイト [`JsEngine`] の背後に隔離する。既定は純Rust の
//! boa（M14）。base.js ロードが遅い（~6.5s/初回）のが問題化したら rquickjs 等を [`JsEngine`]
//! として足し替えるだけでよい（他は無改修）。
//!
//! 現行 base.js は VM 型難読化で nsig 単独関数を切り出せない（PoC U5 で確認）ため、yt-dlp/rustypipe
//! 同様 base.js 全体をロードし、`.get("n")` の URL 書換関数（PoC で `$o6`）を駆動して n を変換する。
//! base.js 版差を吸収するため、関数名と IIFE 終端は実行時に検出して export を注入する。

use anyhow::{anyhow, bail, Result};

const IFRAME_API: &str = "https://www.youtube.com/iframe_api";

/// JS エンジン抽象。これだけが具体エンジン型に触れる境界。
pub trait JsEngine {
    /// スクリプト（stubs + base.js + descramble ラッパ）を一度ロードする。
    fn load(&mut self, script: &str) -> Result<()>;
    /// `descramble(n)` を実行して変換後の n を返す。
    fn call_descramble(&mut self, n: &str) -> Result<String>;
}

/// 純Rust の boa による実装。`Context` は `!Send` なのでワーカースレッドに留める。
pub struct BoaEngine {
    ctx: boa_engine::Context,
}

impl BoaEngine {
    pub fn new() -> Self {
        Self {
            ctx: boa_engine::Context::default(),
        }
    }
}

impl JsEngine for BoaEngine {
    fn load(&mut self, script: &str) -> Result<()> {
        self.ctx
            .eval(boa_engine::Source::from_bytes(script.as_bytes()))
            .map_err(|e| anyhow!("boa: base.js ロード失敗: {e}"))?;
        Ok(())
    }

    fn call_descramble(&mut self, n: &str) -> Result<String> {
        // n は英数字 + _ - のみ（videoplayback の n パラメータ）。念のため検証して JS 注入を防ぐ。
        if !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            bail!("n に不正な文字");
        }
        let code = format!("descramble(\"{n}\")");
        let v = self
            .ctx
            .eval(boa_engine::Source::from_bytes(code.as_bytes()))
            .map_err(|e| anyhow!("boa: descramble 実行失敗: {e}"))?;
        v.as_string()
            .map(|s| s.to_std_string_escaped())
            .ok_or_else(|| anyhow!("boa: descramble の戻り値が文字列でない"))
    }
}

/// nsig 変換器。base.js を遅延ロードして常駐させる（M15）。
pub struct NsigSolver {
    engine: Box<dyn JsEngine>,
    /// ロード済み base.js の player バージョン（変わったら再ロード）。
    loaded_player: Option<String>,
}

impl NsigSolver {
    pub fn new() -> Self {
        Self {
            engine: Box::new(BoaEngine::new()),
            loaded_player: None,
        }
    }

    /// base.js を取得・組立・ロードする（未ロードまたはバージョン変化時）。
    fn ensure_loaded(&mut self, http: &reqwest::blocking::Client) -> Result<()> {
        let (player_id, base_js_url) = base_js_url(http)?;
        if self.loaded_player.as_deref() == Some(player_id.as_str()) {
            return Ok(());
        }
        let base_js = http
            .get(&base_js_url)
            .header("User-Agent", "Mozilla/5.0")
            .send()?
            .error_for_status()?
            .text()?;
        let payload = build_payload(&base_js)?;
        // バージョンが変わった場合は新しい Context が要る（既存に再 eval は不可）。
        self.engine = Box::new(BoaEngine::new());
        self.engine.load(&payload)?;
        self.loaded_player = Some(player_id);
        Ok(())
    }

    /// URL の `n` パラメータを変換して返す。`n` が無ければそのまま返す。
    pub fn transform_url(&mut self, http: &reqwest::blocking::Client, url: &str) -> Result<String> {
        let Some(n) = query_get(url, "n") else {
            return Ok(url.to_string());
        };
        self.ensure_loaded(http)?;
        let new_n = self.engine.call_descramble(&n)?;
        Ok(query_set(url, "n", &new_n))
    }
}

/// iframe_api から player バージョン hash を取り、base.js の URL を組み立てる。
fn base_js_url(http: &reqwest::blocking::Client) -> Result<(String, String)> {
    let api = http
        .get(IFRAME_API)
        .header("User-Agent", "Mozilla/5.0")
        .send()?
        .error_for_status()?
        .text()?;
    // iframe_api は JS 文字列内でスラッシュを `\/` とエスケープする（例: "...player\/445213fb\/www-widgetapi.js"）。
    // 先にエスケープを解いてから player パスを探す。
    let api = api.replace("\\/", "/");
    let marker = "/player/";
    let pos = api
        .find(marker)
        .ok_or_else(|| anyhow!("iframe_api に player パスが見つかりません"))?;
    let rest = &api[pos + marker.len()..];
    let hash: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .collect();
    if hash.is_empty() {
        bail!("player バージョンが取れません");
    }
    let url = format!("https://www.youtube.com/s/player/{hash}/player_ias.vflset/en_US/base.js");
    Ok((hash, url))
}

/// base.js から nsig 駆動用の自己完結スクリプトを組み立てる。
/// stubs + base.js(に `window.__ytnsig=<$o6>;` を注入) + descramble ラッパ。
fn build_payload(base_js: &str) -> Result<String> {
    let o6 = find_o6_name(base_js)
        .ok_or_else(|| anyhow!("nsig 駆動関数($o6 相当)が見つかりません（base.js 仕様変更=U4）"))?;
    // IIFE 終端 `})(...)` の直前に export を注入（$o6 はその位置でスコープ内）。
    let inject_at = base_js
        .rfind("})(")
        .ok_or_else(|| anyhow!("base.js の IIFE 終端が見つかりません"))?;
    let mut injected = String::with_capacity(base_js.len() + STUBS.len() + 256);
    injected.push_str(STUBS);
    injected.push_str(&base_js[..inject_at]);
    injected.push_str(&format!(";window.__ytnsig={o6};"));
    injected.push_str(&base_js[inject_at..]);
    injected.push_str(WRAPPER);
    Ok(injected)
}

/// `.get("n")` を含む URL 書換関数（$o6 相当）の関数名を検出する。
fn find_o6_name(base_js: &str) -> Option<String> {
    let anchor = base_js.find(r#".get("n")"#)?;
    let region = &base_js[..anchor];
    let fpos = region.rfind("=function(")?;
    // `=function(` の直前の識別子を関数名として取り出す。
    let name: String = region[..fpos]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// URL クエリから指定キーの値を取り出す（生のまま=未デコード）。
fn query_get(url: &str, key: &str) -> Option<String> {
    let q = url.split('?').nth(1)?;
    for pair in q.split('&') {
        if let Some(v) = pair.strip_prefix(&format!("{key}=")) {
            return Some(v.to_string());
        }
    }
    None
}

/// URL クエリの指定キーの値を置換する（無ければ追加）。
fn query_set(url: &str, key: &str, value: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, q),
        None => return format!("{url}?{key}={value}"),
    };
    let mut found = false;
    let prefix = format!("{key}=");
    let pairs: Vec<String> = query
        .split('&')
        .map(|p| {
            if p.starts_with(&prefix) {
                found = true;
                format!("{prefix}{value}")
            } else {
                p.to_string()
            }
        })
        .collect();
    let mut q = pairs.join("&");
    if !found {
        q.push_str(&format!("&{prefix}{value}"));
    }
    format!("{base}?{q}")
}

/// バニラ JS エンジン用の最小ブラウザ stub（boa は window/document 等を持たない）。
const STUBS: &str = r#"
var noop=function(){};
function makeEl(){return {setAttribute:noop,getAttribute:function(){return null},appendChild:noop,removeChild:noop,insertBefore:noop,addEventListener:noop,removeEventListener:noop,style:{},classList:{add:noop,remove:noop,contains:function(){return false}},getElementsByTagName:function(){return[]},getElementsByClassName:function(){return[]},querySelector:function(){return null},querySelectorAll:function(){return[]},cloneNode:makeEl,setAttributeNS:noop};}
globalThis.window=globalThis;
globalThis.self=globalThis;
globalThis.document={createElement:makeEl,createTextNode:makeEl,getElementById:function(){return null},getElementsByTagName:function(){return[]},getElementsByClassName:function(){return[]},querySelector:function(){return null},querySelectorAll:function(){return[]},documentElement:makeEl(),body:makeEl(),head:makeEl(),addEventListener:noop,removeEventListener:noop,createEvent:function(){return{initEvent:noop}},cookie:""};
globalThis.navigator={userAgent:"Mozilla/5.0",platform:"Win32",languages:["en"],language:"en",sendBeacon:noop};
globalThis.location={href:"https://www.youtube.com/",protocol:"https:",host:"www.youtube.com",hostname:"www.youtube.com",search:"",hash:""};
globalThis.screen={width:1920,height:1080};
globalThis.setTimeout=globalThis.setTimeout||function(){return 0};
globalThis.clearTimeout=globalThis.clearTimeout||noop;
globalThis.setInterval=globalThis.setInterval||function(){return 0};
globalThis.clearInterval=globalThis.clearInterval||noop;
globalThis.requestAnimationFrame=globalThis.requestAnimationFrame||function(){return 0};
globalThis.XMLHttpRequest=globalThis.XMLHttpRequest||function(){return{open:noop,send:noop,setRequestHeader:noop,addEventListener:noop}};
globalThis.fetch=globalThis.fetch||function(){return{then:function(){return this},catch:function(){return this}}};
globalThis.btoa=globalThis.btoa||function(s){return s};
globalThis.atob=globalThis.atob||function(s){return s};
"#;

/// `window.__ytnsig`（$o6 相当）を /n/<n>/ URL に通して変換後 n を取り出すラッパ。
const WRAPPER: &str = r#"
function descramble(n){
  var p = globalThis.__ytnsig;
  var url = "https://x.googlevideo.com/videoplayback/n/" + n + "/x";
  var out = p(url);
  var m = out.match(/\/n\/([^/]+)/);
  return m ? m[1] : out;
}
"#;
