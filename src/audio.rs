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
/// 録音の最大秒数（録音の上限）。
pub const RECORD_SECS: usize = 3;
/// 録音の最大サンプル数（録音の上限）。
pub const MAX_SAMPLES: usize = SAMPLE_RATE as usize * RECORD_SECS;
/// バッファ全体のサンプル数（録音と再生ダウンロードで共用、再生は最大5秒）。
pub const BUF_SAMPLES: usize = SAMPLE_RATE as usize * 5;

/// 1 回の DMA で扱うサンプル数（<= 4096 バイト = 1024 語）。
const CHUNK: usize = 1024;

// 録音/再生 共用バッファ（内部 RAM の固定領域）。単一タスクからのみ触る。
static mut RECORD_BUF: [i16; BUF_SAMPLES] = [0; BUF_SAMPLES];

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

/// ダウンロードした回答音声を置くバッファ（録音バッファを再利用）。
/// 上限 = MAX_SAMPLES*2 バイト（16kHz で RECORD_SECS 秒）。
pub fn dl_buf() -> &'static mut [u8] {
    // SAFETY: 単一タスクからのみアクセス。録音済みデータは送信後なので上書き可。
    unsafe {
        core::slice::from_raw_parts_mut(
            core::ptr::addr_of_mut!(RECORD_BUF) as *mut u8,
            BUF_SAMPLES * 2,
        )
    }
}

/// 再生用の変換スクラッチ（i32 語）。s16le を 32bit スロットへ載せ替える。
const BLOCK_WORDS: usize = 2048;
static mut PLAY_SCRATCH: [i32; BLOCK_WORDS] = [0; BLOCK_WORDS];

/// `dl_buf()` に入れた s16le PCM（先頭 `byte_len` バイト）をブロック単位で再生する。
///
/// `write_dma_async` のワンショット書き込みを連続で行うシンプル方式。TX は通常
/// モードのままなので、録音（TX 無音でクロック供給）が再生後も正しく動く。
/// 事前に全体をダウンロード済みなので、途中でソケット待ち＝間欠停止は起きない。
pub async fn play_pcm_blocks(i2s_tx: &mut I2sTx<'_, Async>, byte_len: usize) {
    let total = byte_len / 2; // サンプル数
    info!("play: {} bytes ({} samples)", byte_len, total);
    if total == 0 {
        return;
    }

    // PCM のバイト列（RECORD_BUF）を読む。
    let pcm = unsafe {
        core::slice::from_raw_parts(core::ptr::addr_of!(RECORD_BUF) as *const u8, byte_len)
    };
    let scratch = unsafe { &mut *core::ptr::addr_of_mut!(PLAY_SCRATCH) };

    let mut pos = 0usize;
    while pos < total {
        let take = core::cmp::min(scratch.len(), total - pos);
        for j in 0..take {
            let lo = pcm[(pos + j) * 2] as u16;
            let hi = pcm[(pos + j) * 2 + 1] as u16;
            let s = (lo | (hi << 8)) as i16;
            scratch[j] = (s as i32) << 16;
        }
        if i2s_tx
            .write_dma_async(as_bytes_mut(&mut scratch[..take]))
            .await
            .is_err()
        {
            break;
        }
        pos += take;
    }
    info!("play: done");
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

    // RX は 512、TX 無音は RX の 2 倍流す。TX クロックが RX 読み出し中に
    // 途切れないよう、TX を長めにして最後まで供給する。
    let take = core::cmp::min(512, MAX_SAMPLES - filled);
    let mut chunk = [0i32; CHUNK];
    let mut silent = [0i32; CHUNK];
    let tx_len = core::cmp::min(CHUNK, take * 2);

    // TX(無音)でクロックを供給しつつ RX(取り込み)を同時実行。
    // ハードフリーズ防止のためタイムアウトを付ける。
    let joined = embassy_futures::join::join(
        i2s_rx.read_dma_async(as_bytes_mut(&mut chunk[..take])),
        i2s_tx.write_dma_async(as_bytes_mut(&mut silent[..tx_len])),
    );

    match embassy_time::with_timeout(
        embassy_time::Duration::from_millis(300),
        joined,
    )
    .await
    {
        Ok((Ok(()), _)) => {
            // SAFETY: 録音はキーボードタスクからのみ呼ばれ、多重には走らない。
            let buf = unsafe { &mut *core::ptr::addr_of_mut!(RECORD_BUF) };
            for (i, &word) in chunk[..take].iter().enumerate() {
                buf[filled + i] = (word >> 16) as i16;
            }
            filled + take
        }
        Ok((Err(_), _)) => filled,
        Err(_) => {
            info!("capture timeout");
            filled
        }
    }
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
