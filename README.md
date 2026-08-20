# fjcz-netease-qqbot

Rust + OneBot 11 点歌机器人。

在 NapCat 中同时配置：

- HTTP Server：启用，监听例如 127.0.0.1:3000，用于 Rust 调用 OneBot API。
- HTTP Client：目标地址设置为 http://127.0.0.1:8080/onebot，用于 NapCat 上报消息。

OneBot HTTP API 的地址填入 config.toml 的 onebot_api_base，然后执行：

    cp config.example.toml config.toml
    cargo run

首次使用可以先登录网易云账号：

    cargo run -- login-qr

用网易云音乐 App 扫码，成功后 Cookie 会保存到 netease_cookie.txt；之后正常运行 cargo run 即可自动使用登录状态。

配置方式二选一（互斥，不能同时使用）：

- 配置文件方式：`cargo run -- --config ./config.toml`（提供 `--config` 时，所有配置均从该文件读取；再传 `--listen-addr` / `--netease-cookie-file` / `--onebot-api-base` / `--onebot-access-token` 会报错）
- 纯命令行方式：`cargo run -- --onebot-api-base http://127.0.0.1:3000/ [--listen-addr 127.0.0.1:8080] [--netease-cookie-file netease_cookie.txt] [--onebot-access-token xxx]`（不提供 `--config` 时，`--onebot-api-base` 必填，其余有默认值）

登录子命令单独指定 Cookie 文件：

    cargo run -- login-qr --cookie-file ./netease_cookie.txt

把 OneBot HTTP 上报地址设置为 http://127.0.0.1:8080/onebot，在 QQ 中发送“点歌 稻香”。

机器人会通过 ncm-api-rs 搜索歌曲，并使用 OneBot 的 record 消息段发送语音。若适配器不支持直接发送 MP3 URL，需要增加 ffmpeg/SILK 转码。

请遵守音乐服务条款和版权要求。
