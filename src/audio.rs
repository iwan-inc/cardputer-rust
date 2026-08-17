//! I2S マイク録音（ES8311 → I2S RX）。
//!
//! 固定長（既定 3 秒）を 16kHz/mono/s16le で録音し、生 PCM バイト列を返す。
//! PSRAM が無いため録音バッファは内部 RAM の固定 static。
//! 実機で「実音か DC(無音)か」を切り分けられるよう、統計をログ出力する。

use esp_hal::{
    Blocking,
    i2s::master::{I2sRx, I2sTx},
};
use log::info;

pub const SAMPLE_RATE: u32 = 16000;
/// 録音の最大秒数。RAM に直結（16kHz*2byte*秒）。
pub const RECORD_SECS: usize = 3;
/// 録音バッファのサンプル数。
pub const MAX_SAMPLES: usize = SAMPLE_RATE as usize * RECORD_SECS;

// 録音バッファ（内部 RAM の固定領域）。単一タスクからのみ触る。
static mut RECORD_BUF: [i16; MAX_SAMPLES] = [0; MAX_SAMPLES];

/// 1 チャンク（最大 1024 サンプル ≈ 64ms）を録音バッファの `filled` 以降へ
/// 取り込み、新しい `filled` を返す（バッファ上限で頭打ち）。
///
/// I2S は 32bit スロット。16bit データはスロット上位（[31:16]）に載るので
/// 上位 16bit を取り出す。`read_words` は 1 回最大 4096 バイト = 1024 語。
/// 1 回のブロッキングは約 64ms。押している間、ループから繰り返し呼ぶ。
pub fn capture_chunk(i2s_rx: &mut I2sRx<'_, Blocking>, filled: usize) -> usize {
    if filled >= MAX_SAMPLES {
        return filled;
    }

    // SAFETY: 録音はキーボードタスクからのみ呼ばれ、多重には走らない。
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RECORD_BUF) };

    let mut chunk = [0i32; 1024];
    let take = core::cmp::min(chunk.len(), MAX_SAMPLES - filled);

    if i2s_rx.read_words(&mut chunk[..take]).is_err() {
        return filled;
    }

    for (i, &word) in chunk[..take].iter().enumerate() {
        buf[filled + i] = (word >> 16) as i16;
    }

    filled + take
}

/// I2S TX を無音で少し回して初回起動のグリッチ（雑音）を吸収する。
/// 起動時に一度呼ぶ。
pub fn prime(i2s_tx: &mut I2sTx<'_, Blocking>) {
    let silent = [0i32; 1024];
    for _ in 0..4 {
        if i2s_tx.write_words(&silent).is_err() {
            break;
        }
    }
}

/// スピーカー検証用に矩形波のテスト音を鳴らす（`freq_hz`, `ms` ミリ秒）。
///
/// I2S は 32bit スロット。16bit サンプルを上位に載せる（sample << 16）。
/// `write_words` は 1 回最大 4096 バイト = 1024 語。
pub fn play_tone(i2s_tx: &mut I2sTx<'_, Blocking>, freq_hz: u32, ms: u32) {
    const AMP: i16 = 8000;
    let half = (SAMPLE_RATE / freq_hz / 2).max(1); // 半周期のサンプル数
    let total = (SAMPLE_RATE as u64 * ms as u64 / 1000) as usize;

    let mut chunk = [0i32; 1024];
    let mut phase = 0u32;
    let mut done = 0;

    info!("play_tone {}Hz {}ms", freq_hz, ms);

    while done < total {
        let n = core::cmp::min(chunk.len(), total - done);
        for slot in chunk.iter_mut().take(n) {
            let s = if phase < half { AMP } else { -AMP };
            *slot = (s as i32) << 16;
            phase += 1;
            if phase >= half * 2 {
                phase = 0;
            }
        }
        if i2s_tx.write_words(&chunk[..n]).is_err() {
            break;
        }
        done += n;
    }
}

/// 録音済み `filled` サンプルを PCM（s16le）バイトとして返す。統計もログ出力。
pub fn pcm_bytes(filled: usize) -> &'static [u8] {
    // SAFETY: 単一タスクからのみアクセスする。
    let buf = unsafe { &*core::ptr::addr_of!(RECORD_BUF) };

    // 実機デバッグ用の統計（DC/無音なら min≈max、クリップなら ±32767 に張り付く）。
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

    // s16le の生バイト（ESP32 はリトルエンディアン）。
    unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u8, filled * 2) }
}
