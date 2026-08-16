//! 端末の設定値を一元管理するモジュール。
//!
//! ネットワークや接続先を変えるときは、原則ここだけを編集する。
//! Wi-Fi 認証情報などの秘密情報は git 管理外の `secrets` モジュールにある。

use embassy_net::Ipv4Address;

// ---- Wi-Fi (STA) ----
// 秘密情報は `src/secrets.rs`（.gitignore 済み）で定義し、ここで再公開する。
pub use crate::secrets::{WIFI_PASSWORD, WIFI_SSID};

// ---- 接続先サーバー ----

/// 接続先サーバーの IPv4 アドレス。
pub const SERVER_IP: Ipv4Address = Ipv4Address::new(192, 168, 10, 53);

/// 接続先サーバーの TCP ポート。
pub const SERVER_PORT: u16 = 18080;

/// HTTP の Host ヘッダに使う文字列。
///
/// `SERVER_IP:SERVER_PORT` と一致させること（no_std では実行時に
/// 整形しづらいため、あえて文字列としても持っている）。
pub const SERVER_HOST: &str = "192.168.10.53:18080";

/// GET で取得するパス。
pub const REQUEST_PATH: &str = "/hello.txt";
