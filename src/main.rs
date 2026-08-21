use anyhow::{Context, Result, bail};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use clap::{Parser, Subcommand};
use ncm_api_rs::{ApiClient, Query, create_client};
use qrcode::QrCode;
use qrcode::render::unicode;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::net::TcpListener;
use tokio::time::{Duration, sleep};
use tracing::{error, info};

#[derive(Debug, Parser)]
#[command(
    name = "fjcz-netease-qqbot",
    version,
    about = "OneBot 11 网易云点歌机器人"
)]
struct Cli {
    /// 配置文件路径；与 --listen-addr / --netease-cookie-file / --onebot-api-base / --onebot-access-token 互斥
    #[arg(
        long,
        global = true,
        conflicts_with_all = ["listen_addr", "netease_cookie_file", "onebot_api_base", "onebot_access_token"]
    )]
    config: Option<PathBuf>,

    /// 监听地址（仅在未指定 --config 时生效）
    #[arg(short = 'l', long, default_value = "127.0.0.1:8080")]
    listen_addr: String,

    /// 网易云 Cookie 文件（仅在未指定 --config 时生效）
    #[arg(short = 'n', long, default_value = "netease_cookie.txt")]
    netease_cookie_file: String,

    /// OneBot API 地址（仅在未指定 --config 时必填）
    #[arg(short = 'b', long)]
    onebot_api_base: Option<String>,

    /// OneBot 访问令牌（仅在未指定 --config 时生效）
    #[arg(short = 't', long)]
    onebot_access_token: Option<String>,

    #[command(subcommand)]
    command: Option<CommandArgs>,
}

#[derive(Debug, Subcommand)]
enum CommandArgs {
    /// 使用网易云 App 扫码登录并保存 Cookie
    LoginQr {
        #[arg(short, long, default_value = "netease_cookie.txt")]
        cookie_file: PathBuf,
    },
}

#[derive(Clone, Deserialize)]
struct Config {
    listen_addr: String,
    onebot_api_base: String,
    #[serde(default)]
    onebot_access_token: Option<String>,
    netease_cookie_file: String,
}

#[derive(Clone)]
struct AppState {
    config: Arc<Config>,
    client: Client,
    ncm: ApiClient,
}

#[derive(Deserialize)]
struct Event {
    post_type: Option<String>,
    message_type: Option<String>,
    raw_message: Option<String>,
    message: Option<Value>,
    user_id: Option<i64>,
    group_id: Option<i64>,
}

#[derive(Serialize)]
struct Segment {
    #[serde(rename = "type")]
    kind: &'static str,
    data: Value,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let cli = Cli::parse();

    if let Some(CommandArgs::LoginQr { cookie_file }) = cli.command {
        return login_qr(&cookie_file).await;
    }

    let config = match &cli.config {
        Some(path) => {
            let text = tokio::fs::read_to_string(path)
                .await
                .context("请复制 config.example.toml 为 config.toml")?;
            toml::from_str(&text).context("config.toml 格式错误")?
        }
        None => Config {
            listen_addr: cli.listen_addr.clone(),
            onebot_api_base: cli.onebot_api_base.clone().context(
                "请通过 --config 指定配置文件，或通过 --onebot-api-base 指定 OneBot API 地址",
            )?,
            onebot_access_token: cli.onebot_access_token.clone(),
            netease_cookie_file: cli.netease_cookie_file.clone(),
        },
    };

    info!(
        onebot_token_configured = config
            .onebot_access_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty()),
        "loaded configuration"
    );

    let config = Arc::new(config);
    let addr = &config.clone().listen_addr;
    let cookie = tokio::fs::read_to_string(&config.netease_cookie_file)
        .await
        .ok()
        .filter(|cookie| !cookie.trim().is_empty());

    let state = AppState {
        config,
        client: Client::builder().user_agent("qq-music-bot/0.1").build()?,
        ncm: create_client(cookie),
    };

    let app = Router::new()
        .route("/", post(webhook))
        .route("/onebot", post(webhook))
        .with_state(state);

    let listener = TcpListener::bind(addr).await?;
    info!("listening on http://{addr}/onebot");
    axum::serve(listener, app).await?;

    Ok(())
}

async fn login_qr(cookie_file: &Path) -> Result<()> {
    let ncm = create_client(None);
    let key_response = ncm.login_qr_key(&Query::new()).await?;
    let key = key_response.body["data"]["unikey"]
        .as_str()
        .or_else(|| key_response.body["unikey"].as_str())
        .context("网易云没有返回二维码 key")?
        .to_owned();

    let qr_url = format!("https://music.163.com/login?codekey={key}");

    println!("请使用网易云音乐 App 扫描下面的二维码：\n");

    match QrCode::new(qr_url.as_bytes()) {
        Ok(code) => {
            let image = code.render::<unicode::Dense1x2>().quiet_zone(true).build();
            println!("{image}");
        }
        Err(_) => println!("二维码地址：{qr_url}"),
    }

    println!("\n等待扫码登录...");

    loop {
        let response = ncm.login_qr_check(&Query::new().param("key", &key)).await?;
        let code = response.body["code"].as_i64().unwrap_or_default();
        match code {
            800 => bail!("二维码已失效，请重新执行 cargo run -- login-qr"),
            803 => {
                let cookie = response.cookie.join("; ");

                if cookie.is_empty() {
                    bail!("登录成功但网易云没有返回 Cookie");
                }

                tokio::fs::write(cookie_file, &cookie)
                    .await
                    .context("保存网易云 Cookie 失败")?;

                println!("登录成功，Cookie 已保存到 {}", cookie_file.display());

                return Ok(());
            }
            802 => println!("已扫码，请在网易云 App 中确认登录..."),
            _ => {}
        }
        sleep(Duration::from_secs(2)).await;
    }
}

async fn webhook(State(state): State<AppState>, Json(event): Json<Event>) -> StatusCode {
    info!(
        post_type = ?event.post_type,
        message_type = ?event.message_type,
        raw_message = ?event.raw_message,
        "received OneBot event"
    );

    if event.post_type.as_deref() != Some("message") {
        return StatusCode::OK;
    }

    let text = event
        .raw_message
        .clone()
        .or_else(|| event.message.as_ref().and_then(message_text))
        .unwrap_or_default();

    let text = text.trim();
    let Some(keyword) = text.strip_prefix("点歌").map(str::trim) else {
        return StatusCode::OK;
    };

    if keyword.is_empty() {
        return StatusCode::OK;
    }

    let keyword = keyword.to_owned();
    tokio::spawn(async move {
        if let Err(e) = process(&state, &event, &keyword).await {
            error!(?e, "点歌失败");
        }
    });

    StatusCode::OK
}

fn message_text(message: &Value) -> Option<String> {
    if let Some(text) = message.as_str() {
        return Some(text.to_owned());
    }

    let segments = message.as_array()?;
    let text = segments
        .iter()
        .filter(|segment| segment["type"].as_str() == Some("text"))
        .filter_map(|segment| segment["data"]["text"].as_str())
        .collect::<String>();

    Some(text)
}

async fn process(state: &AppState, event: &Event, keyword: &str) -> Result<()> {
    let query = Query::new()
        .param("keywords", keyword)
        .param("limit", "1")
        .param("type", "1");

    let body: Value = state.ncm.cloudsearch(&query).await?.body;

    let Some(song) = body["result"]["songs"].as_array().and_then(|v| v.first()) else {
        return send_text(state, event, &format!("没有找到歌曲：{keyword}")).await;
    };

    let id = song["id"].as_i64().context("搜索结果缺少歌曲 ID")?;
    let name = song["name"].as_str().unwrap_or("未知歌曲").to_owned();
    let artist = song["ar"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|a| a["name"].as_str())
                .collect::<Vec<_>>()
                .join("、")
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "未知歌手".to_owned());

    let id_text = id.to_string();
    let query = Query::new()
        .param("id", &id_text)
        .param("level", "standard");

    let url: Value = state.ncm.song_url_v1(&query).await?.body;

    let Some(audio) = url["data"]
        .as_array()
        .and_then(|v| v.first())
        .and_then(|v| v["url"].as_str())
    else {
        return send_text(state, event, &format!("《{name}》暂无可用播放地址。")).await;
    };
    info!(%name, %artist, "send record");

    send(
        state,
        event,
        vec![
            Segment {
                kind: "record",
                data: json!({"file": audio}),
            },
            Segment {
                kind: "text",
                data: json!({"text": format!("{name} - {artist}")}),
            },
        ],
    )
    .await
}

async fn send_text(state: &AppState, event: &Event, text: &str) -> Result<()> {
    send(
        state,
        event,
        vec![Segment {
            kind: "text",
            data: json!({"text": text}),
        }],
    )
    .await
}

async fn send(state: &AppState, event: &Event, message: Vec<Segment>) -> Result<()> {
    let kind = event.message_type.as_deref().unwrap_or("private");
    let mut payload = json!({"message_type": kind, "message": message});

    match kind {
        "group" => payload["group_id"] = json!(event.group_id.context("缺少 group_id")?),
        "private" => payload["user_id"] = json!(event.user_id.context("缺少 user_id")?),
        other => bail!("不支持的消息类型：{other}"),
    }

    let mut req = state
        .client
        .post(api_url(&state.config.onebot_api_base, "send_msg")?)
        .json(&payload);

    if let Some(token) = &state.config.onebot_access_token {
        req = req.bearer_auth(token.trim());
    }

    let response = req.send().await.context("请求 OneBot send_msg 失败")?;
    let status = response.status();

    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("OneBot send_msg 返回 {status}: {body}");
    }

    Ok(())
}

fn api_url(base: &str, path: &str) -> Result<Url> {
    let base = if base.ends_with('/') {
        base.to_owned()
    } else {
        format!("{base}/")
    };

    Ok(Url::parse(&base)?.join(path.trim_start_matches('/'))?)
}
