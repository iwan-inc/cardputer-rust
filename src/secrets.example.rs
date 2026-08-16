//! 秘密情報（Wi-Fi 認証情報など）のテンプレート。
//!
//! このファイルを `secrets.rs` にコピーし、自分の環境の値に書き換えること。
//! `secrets.rs` は `.gitignore` で git 管理外になっている。

/// 接続先アクセスポイントの SSID。
pub const WIFI_SSID: &str = "your-wifi-ssid";

/// 接続先アクセスポイントのパスワード。
pub const WIFI_PASSWORD: &str = "your-wifi-password";
