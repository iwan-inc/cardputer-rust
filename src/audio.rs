//! I2S マイク録音・スピーカー再生（ES8311、全二重）。
//!
//! マイクとスピーカーは同じ ES8311 のクロック（BCLK/WS）を共有する。
//! `signal_loopback` で RX は TX のクロックに従属するため、録音時は
//! TX に無音を流してクロックを供給しながら RX を読む（同時実行）。
//! 再生は TX に音声を書くだけ（TX マスターがクロックを駆動）。
//!
//! 16kHz / mono / s16le。I2S は 32bit スロットで 16bit を上位に載せる。

use esp_hal::{
    Async,
    i2s::master::{I2sRx, I2sTx},
};
use log::info;

pub const SAMPLE_RATE: u32 = 16000;
/// 録音の最大秒数。RAM に直結（16kHz*2byte*秒）。
pub const RECORD_SECS: usize = 3;
/// 録音バッファのサンプル数。
pub const MAX_SAMPLES: usize = SAMPLE_RATE as usize * RECORD_SECS;

/// 1 回の DMA で扱うサンプル数（<= 4096 バイト = 1024 語）。
const CHUNK: usize = 1024;

// 録音バッファ（内部 RAM の固定領域）。単一タスクからのみ触る。
static mut RECORD_BUF: [i16; MAX_SAMPLES] = [0; MAX_SAMPLES];

/// i32 バッファをバイトスライスとして見る（DMA API 用）。
fn as_bytes_mut(buf: &mut [i32]) -> &mut [u8] {
    // SAFETY: i32 スライスを同じ領域の u8 スライスとして見るだけ。
    unsafe {
        core::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, buf.len() * 4)
    }
}

/// TX に無音を少し流して初回起動のグリッチ（雑音）を吸収する。起動時に呼ぶ。
pub async fn prime(i2s_tx: &mut I2sTx<'_, Async>) {
    let mut silent = [0i32; CHUNK];
    for _ in 0..4 {
        if i2s_tx.write_dma_async(as_bytes_mut(&mut silent)).await.is_err() {
            break;
        }
    }
}

/// スピーカー検証用に矩形波のテスト音を鳴らす（`freq_hz`, `ms` ミリ秒）。
pub async fn play_tone(i2s_tx: &mut I2sTx<'_, Async>, freq_hz: u32, ms: u32) {
    const AMP: i16 = 8000;
    let half = (SAMPLE_RATE / freq_hz / 2).max(1);
    let total = (SAMPLE_RATE as u64 * ms as u64 / 1000) as usize;

    let mut chunk = [0i32; CHUNK];
    let mut phase = 0u32;
    let mut done = 0;

    info!("play_tone {}Hz {}ms", freq_hz, ms);

    while done < total {
        let n = core::cmp::min(CHUNK, total - done);
        for slot in chunk.iter_mut().take(n) {
            let s = if phase < half { AMP } else { -AMP };
            *slot = (s as i32) << 16;
            phase += 1;
            if phase >= half * 2 {
                phase = 0;
            }
        }
        if i2s_tx
            .write_dma_async(as_bytes_mut(&mut chunk[..n]))
            .await
            .is_err()
        {
            break;
        }
        done += n;
    }
}

/// s16le の PCM バイト列を再生する（16kHz/mono）。奇数バイトは無視。
pub async fn play_pcm(i2s_tx: &mut I2sTx<'_, Async>, pcm: &[u8]) {
    let mut chunk = [0i32; CHUNK];
    let mut i = 0;
    let samples = pcm.len() / 2;

    while i < samples {
        let n = core::cmp::min(CHUNK, samples - i);
        for (j, slot) in chunk.iter_mut().take(n).enumerate() {
            let lo = pcm[(i + j) * 2] as i16;
            let hi = pcm[(i + j) * 2 + 1] as i16;
            let s = (lo & 0xFF) | (hi << 8);
            *slot = (s as i32) << 16;
        }
        if i2s_tx
            .write_dma_async(as_bytes_mut(&mut chunk[..n]))
            .await
            .is_err()
        {
            break;
        }
        i += n;
    }
}

/// 1 チャンク録音して録音バッファの `filled` 以降へ書き、新しい `filled` を返す。
///
/// RX は TX クロックに従属するので、TX に無音を流しながら RX を読む
/// （`join` で同時実行）。I2S は 32bit スロットなので上位 16bit を取り出す。
pub async fn capture_chunk(
    i2s_rx: &mut I2sRx<'_, Async>,
    i2s_tx: &mut I2sTx<'_, Async>,
    filled: usize,
) -> usize {
    if filled >= MAX_SAMPLES {
        return filled;
    }

    let take = core::cmp::min(CHUNK, MAX_SAMPLES - filled);
    let mut chunk = [0i32; CHUNK];
    let mut silent = [0i32; CHUNK];

    // TX(無音)でクロックを供給しつつ RX(取り込み)を同時実行。
    let (rx_res, _tx_res) = embassy_futures::join::join(
        i2s_rx.read_dma_async(as_bytes_mut(&mut chunk[..take])),
        i2s_tx.write_dma_async(as_bytes_mut(&mut silent[..take])),
    )
    .await;

    if rx_res.is_err() {
        return filled;
    }

    // SAFETY: 録音はキーボードタスクからのみ呼ばれ、多重には走らない。
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(RECORD_BUF) };
    for (i, &word) in chunk[..take].iter().enumerate() {
        buf[filled + i] = (word >> 16) as i16;
    }

    filled + take
}

/// 録音済み `filled` サンプルを PCM（s16le）バイトとして返す。統計もログ出力。
pub fn pcm_bytes(filled: usize) -> &'static [u8] {
    // SAFETY: 単一タスクからのみアクセスする。
    let buf = unsafe { &*core::ptr::addr_of!(RECORD_BUF) };

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

    unsafe { core::slice::from_raw_parts(buf.as_ptr() as *const u8, filled * 2) }
}
