//! Wi-Fi (STA) の接続設定ビルダとスキャン。

use alloc::vec::Vec;

use esp_radio::wifi::{
    Config as WifiConfig,
    ControllerConfig,
    WifiController,
    WifiError,
    ap::AccessPointInfo,
    scan::ScanConfig,
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

/// 周囲のアクセスポイントをスキャンし、最大 `max` 件を返す。
pub async fn scan(
    controller: &mut WifiController<'_>,
    max: usize,
) -> Result<Vec<AccessPointInfo>, WifiError> {
    let config = ScanConfig::default().with_max(max);
    controller.scan_async(&config).await
}
