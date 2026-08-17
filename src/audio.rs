//! I2S マイク録音（ES8311 → I2S RX）。
//!
//! 固定長（既定 3 秒）を 16kHz/mono/s16le で録音し、生 PCM バイト列を返す。
//! PSRAM が無いため録音バッファは内部 RAM の固定 static。
//! 実機で「実音か DC(無音)か」を切り分けられるよう、統計をログ出力する。

use esp_hal::{
    Blocking,
    i2s::master::I2sRx,
};
use log::info;

pub const SAMPLE_RATE: u32 = 16000;
/// 録音の最大秒数。RAM に直結（16kHz*2byte*秒）。
pub const RECORD_SECS: usize = 3;
/// 録音バッファのサンプル数。
pub const MAX_SAMPLES: usize = SAMPLE_RATE as usize * RECORD_SECS;

// 録音バッファ（内部 RAM の固定領域）。単一タスクからのみ触る。
static mut RECORD_BUF: [i16; MAX_SAMPLES] = [0; MAX_SAMPLES];

/// 固定長を録音し、録音済み PCM（s16le）のバイトスライスを返す。
///
/// `read_words` は 1 回最大 4096 バイト（2048 サンプル）なのでチャンク読みする。
/// チャンク毎に少し await して、同居するネットワークタスクにも実行を譲る。
pub async fn record(i2s_rx: &mut I2sRx<'_, Blocking>) -> &'static [u8] {
    // SAFETY: 録音はキーボードタスクからのみ呼ばれ、多重には走らない。
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RECORD_BUF) };

    // I2S は 32bit スロット。16bit データはスロット上位（[31:16]）に載るので
    // 上位 16bit を取り出す。read_words は 1 回最大 4096 バイト = 1024 語。
    let mut chunk = [0i32; 1024];
    let mut filled = 0;

    while filled < MAX_SAMPLES {
        let take = core::cmp::min(chunk.len(), MAX_SAMPLES - filled);

        if i2s_rx.read_words(&mut chunk[..take]).is_err() {
            break;
        }

        for (i, &word) in chunk[..take].iter().enumerate() {
            buf[filled + i] = (word >> 16) as i16;
        }
        filled += take;

        // ブロッキング読みの合間に executor へ譲る。
        embassy_time::Timer::after_millis(1).await;
    }

    // 実機デバッグ用の統計（DC/無音なら min≈max になる）。
    let mut min = i16::MAX;
    let mut max = i16::MIN;
    let mut sum: i64 = 0;
    for &s in &buf[..filled] {
        if s < min {
            min = s;
        }
        if s > max {
            max = s;
        }
        sum += s as i64;
    }
    let mean = if filled > 0 { sum / filled as i64 } else { 0 };
    info!(
        "audio: {} samples, min={} max={} mean={}",
        filled, min, max, mean
    );

    // s16le の生バイトとして返す（ESP32 はリトルエンディアン）。
    unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u8, filled * 2) }
}
