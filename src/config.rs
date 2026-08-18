//! 端末の設定値を一元管理するモジュール。
//!
//! ネットワークや接続先を変えるときは、原則ここだけを編集する。
//! Wi-Fi 認証情報などの秘密情報は git 管理外の `secrets` モジュールにある。

// ---- Wi-Fi (STA) と接続先サーバー ----
// 環境依存の値（Wi-Fi 認証情報・LAN 内サーバーのアドレス）は
// `src/secrets.rs`（.gitignore 済み）で定義し、ここで再公開する。
// テンプレートは `src/secrets.example.rs`。
pub use crate::secrets::{
    SERVER_HOST, SERVER_IP, SERVER_PORT, WIFI_PASSWORD, WIFI_SSID,
};

/// GET で取得するパス。
pub const REQUEST_PATH: &str = "/hello.txt";

/// 入力文字列を POST 送信するパス（テキスト echo）。
pub const POST_PATH: &str = "/msg";

/// 入力を送り、描画済み 1bit 画像を受け取るパス（日本語対応）。
pub const RENDER_PATH: &str = "/render";

/// 録音音声を送り、文字起こし結果の画像を受け取るパス。
pub const STT_PATH: &str = "/stt";

/// 録音音声を送り、文字起こし→AI回答の画像を受け取るパス。
pub const ASK_PATH: &str = "/ask";

/// 直近の回答音声（生 PCM）を取得するパス（GET）。
pub const SPEAK_PATH: &str = "/speak";
