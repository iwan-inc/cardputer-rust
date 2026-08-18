//! 秘密情報（Wi-Fi 認証情報・接続先サーバーなど）のテンプレート。
//!
//! このファイルを `secrets.rs` にコピーし、自分の環境の値に書き換えること。
//! `secrets.rs` は `.gitignore` で git 管理外になっている。

use embassy_net::Ipv4Address;

/// 接続先アクセスポイントの SSID。
pub const WIFI_SSID: &str = "your-wifi-ssid";

/// 接続先アクセスポイントのパスワード。
pub const WIFI_PASSWORD: &str = "your-wifi-password";

/// 接続先サーバーの IPv4 アドレス（LAN 内のサーバー）。
pub const SERVER_IP: Ipv4Address = Ipv4Address::new(192, 168, 0, 10);

/// 接続先サーバーの TCP ポート。
pub const SERVER_PORT: u16 = 18080;

/// HTTP の Host ヘッダ文字列。`SERVER_IP:SERVER_PORT` と一致させること
/// （no_std では実行時整形しづらいため文字列としても持つ）。
pub const SERVER_HOST: &str = "192.168.0.10:18080";
