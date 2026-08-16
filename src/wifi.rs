//! Wi-Fi (STA) の接続設定ビルダ。

use esp_radio::wifi::{
    Config as WifiConfig,
    ControllerConfig,
    sta::StationConfig,
};

/// 指定した SSID / パスワードで STA モードの `ControllerConfig` を組み立てる。
pub fn controller_config(ssid: &str, password: &str) -> ControllerConfig {
    let station_config = WifiConfig::Station(
        StationConfig::default()
            .with_ssid(ssid)
            .with_password(password.into()),
    );

    ControllerConfig::default().with_initial_config(station_config)
}
