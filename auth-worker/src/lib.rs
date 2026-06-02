//! YouTube Super Lite 用 OAuth トークン交換 Worker（Cloudflare Workers / workers-rs）。
//!
//! client_secret は Worker の Secret としてのみ存在し、配布アプリには渡さない。
//! デスクトップから受け取った認可コード / リフレッシュトークンに、ここで
//! client_id/secret を付けて Google のトークンエンドポイントへ中継する。
//!
//! 設定:
//!   vars   GAUTH_CLIENT_ID      （wrangler.jsonc の vars）
//!   secret GAUTH_CLIENT_SECRET  （`wrangler secret put GAUTH_CLIENT_SECRET`）
//!
//! エンドポイント:
//!   GET  /client_id  -> { "client_id": "..." }
//!   POST /token      <- { "code": "...", "redirect_uri": "..." }  -> Google のトークン JSON
//!   POST /refresh    <- { "refresh_token": "..." }                -> Google のトークン JSON

use worker::*;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    Router::new()
        .get_async("/client_id", |_req, ctx| async move {
            let client_id = ctx.env.var("GAUTH_CLIENT_ID")?.to_string();
            Response::from_json(&serde_json::json!({ "client_id": client_id }))
        })
        .post_async("/token", |req, ctx| async move { relay(req, ctx, true).await })
        .post_async("/refresh", |req, ctx| async move {
            relay(req, ctx, false).await
        })
        .run(req, env)
        .await
}

/// 認可コード / リフレッシュトークンを Google のトークンエンドポイントへ中継する。
async fn relay(mut req: Request, ctx: RouteContext<()>, is_code: bool) -> Result<Response> {
    let body: serde_json::Value = req.json().await.unwrap_or(serde_json::json!({}));

    let client_id = ctx.env.var("GAUTH_CLIENT_ID")?.to_string();
    let client_secret = ctx.env.secret("GAUTH_CLIENT_SECRET")?.to_string();

    let mut form = format!(
        "client_id={}&client_secret={}",
        enc(&client_id),
        enc(&client_secret)
    );

    if is_code {
        let code = body["code"].as_str().unwrap_or("");
        let redirect = body["redirect_uri"].as_str().unwrap_or("");
        if code.is_empty() || redirect.is_empty() {
            return Response::error("code と redirect_uri が必要です", 400);
        }
        form.push_str(&format!(
            "&grant_type=authorization_code&code={}&redirect_uri={}",
            enc(code),
            enc(redirect)
        ));
    } else {
        let rt = body["refresh_token"].as_str().unwrap_or("");
        if rt.is_empty() {
            return Response::error("refresh_token が必要です", 400);
        }
        form.push_str(&format!(
            "&grant_type=refresh_token&refresh_token={}",
            enc(rt)
        ));
    }

    let mut headers = Headers::new();
    headers.set("Content-Type", "application/x-www-form-urlencoded")?;

    let mut init = RequestInit::new();
    init.with_method(Method::Post)
        .with_headers(headers)
        .with_body(Some(form.into()));

    let google_req = Request::new_with_init(TOKEN_URL, &init)?;
    let mut resp = Fetch::Request(google_req).send().await?;

    let status = resp.status_code();
    let text = resp.text().await?;

    // Google のレスポンス（JSON）をそのまま、ステータスを保ったまま返す。
    Ok(Response::ok(text)?.with_status(status))
}

/// application/x-www-form-urlencoded 用の最小パーセントエンコード。
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
