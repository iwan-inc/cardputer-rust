//! 端末のキーボード入力ループ。
//!
//! キーボード (TCA8418) を読み、打鍵した文字を LCD に表示し続ける。
//! ネットワークタスクと同じ executor 上で共存させるため、
//! 各周回で短時間スリープして処理を譲る。
//!
//! 機能:
//! - キーリピート（押しっぱなしで連続入力）
//! - 行をまたぐ Backspace（行頭で押すと前行末尾を削除）
//! - Fn + Backspace で全消去（先頭行へ）

use embassy_net::{
    Stack,
    tcp::TcpSocket,
};
use embedded_graphics::{
    mono_font::MonoTextStyle,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::Text,
};
use embedded_hal::i2c::I2c;
use esp_hal::{
    Async,
    i2s::master::{I2sRx, I2sTx},
    system::software_reset,
};
use esp_radio::wifi::WifiController;
use tca8418::Tca8418;

use crate::{audio, config, keyboard, net, ui, wifi};

// レイアウト定数（画面 240x135, FONT_10X20）
const CHAR_W: i32 = 10;
const CHAR_H: i32 = 20;
const LEFT: i32 = 10;
const RIGHT: i32 = 230;
const TOP: i32 = 20;
/// 画面に収まる行数（y = TOP + 行*CHAR_H）。
const MAX_LINES: usize = 6;
/// 送信メッセージ（現在の入力）の最大バイト数。
const MSG_CAP: usize = 128;

// 画面（LCD）の解像度。サーバの画像サイズと一致させること。
const SCREEN_W: u32 = 240;
const SCREEN_H: u32 = 135;
/// 応答（1bit 画像 + HTTP ヘッダ）用の受信バッファ容量。
/// 240x135/8 = 4050 バイト + ヘッダに余裕を持たせる。
const RESPONSE_CAP: usize = 6144;

// キーリピートのタイミング（1 周回 = 約 20ms）
/// 押してから最初のリピートまでの周回数（約 400ms）。
const REPEAT_DELAY_TICKS: u32 = 20;
/// リピート間隔の周回数（約 80ms）。
const REPEAT_INTERVAL_TICKS: u32 = 4;

/// カーソル位置と各行の終端 x、および送信用の入力バッファを管理する。
struct Editor {
    cursor_x: i32,
    cursor_y: i32,
    /// 現在行（0..MAX_LINES）。
    line: usize,
    /// 各行の「次に文字を置く x」（＝入力済みの終端）。
    line_end: [i32; MAX_LINES],
    /// 送信するメッセージ（前回 Enter/クリア以降に打った文字）。
    buf: [u8; MSG_CAP],
    buf_len: usize,
}

impl Editor {
    fn new() -> Self {
        Self {
            cursor_x: LEFT,
            cursor_y: TOP,
            line: 0,
            line_end: [LEFT; MAX_LINES],
            buf: [0u8; MSG_CAP],
            buf_len: 0,
        }
    }

    /// 現在の入力メッセージ。
    fn message(&self) -> &[u8] {
        &self.buf[..self.buf_len]
    }

    /// 画面はそのままに、カーソルと入力バッファを初期状態へ戻す。
    fn reset(&mut self) {
        self.cursor_x = LEFT;
        self.cursor_y = TOP;
        self.line = 0;
        self.line_end = [LEFT; MAX_LINES];
        self.buf_len = 0;
    }

    /// 画面はそのままに、カーソルを最下行へ移す（受信内容や一覧は上部に
    /// 出るため、カーソルが重ならないようにする）。
    fn reset_to_bottom(&mut self) {
        self.reset();
        self.line = MAX_LINES - 1;
        self.cursor_y = TOP + (MAX_LINES as i32 - 1) * CHAR_H;
    }

    /// 次の行へ移動する。最終行では折り返さず先頭に戻る（簡易）。
    fn advance_line(&mut self) {
        if self.line + 1 < MAX_LINES {
            self.line += 1;
            self.cursor_y += CHAR_H;
            self.cursor_x = LEFT;
            self.line_end[self.line] = LEFT;
        } else {
            // 最終行の末尾: 先頭に戻して上書き継続。
            self.cursor_x = LEFT;
            self.line_end[self.line] = LEFT;
        }
    }

    /// 1 文字描画してカーソルを進める。右端で自動改行。
    fn put_char<D>(&mut self, display: &mut D, style: MonoTextStyle<'_, Rgb565>, ch: char)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let mut buf = [0u8; 4];
        let s = ch.encode_utf8(&mut buf);

        let _ = Text::new(s, Point::new(self.cursor_x, self.cursor_y), style)
            .draw(display);

        // 送信バッファへ追記（ASCII のみ、あふれたら無視）。
        if ch.is_ascii() && self.buf_len < MSG_CAP {
            self.buf[self.buf_len] = ch as u8;
            self.buf_len += 1;
        }

        self.cursor_x += CHAR_W;
        self.line_end[self.line] = self.cursor_x;

        if self.cursor_x > RIGHT - CHAR_W {
            self.advance_line();
        }
    }

    /// 改行する。入力バッファも新しい行としてリセットする。
    fn newline(&mut self) {
        self.line_end[self.line] = self.cursor_x;
        self.advance_line();
        self.buf_len = 0;
    }

    /// 指定セルを黒で塗ってカーソルをそこへ置く。
    fn erase_cell<D>(&mut self, display: &mut D)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let _ = Rectangle::new(
            Point::new(self.cursor_x, self.cursor_y - CHAR_H + 4),
            Size::new(CHAR_W as u32, CHAR_H as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(display);

        self.line_end[self.line] = self.cursor_x;
    }

    /// 1 文字削除。行頭では前行の末尾へ回り込んで削除する。
    fn backspace<D>(&mut self, display: &mut D)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // 送信バッファからも 1 文字取り除く。
        if self.buf_len > 0 {
            self.buf_len -= 1;
        }

        if self.cursor_x > LEFT {
            self.cursor_x -= CHAR_W;
            self.erase_cell(display);
        } else if self.line > 0 {
            // 前行の末尾へ移動。
            self.line -= 1;
            self.cursor_y -= CHAR_H;
            self.cursor_x = self.line_end[self.line];

            // 前行に文字があれば 1 文字削除する。
            if self.cursor_x > LEFT {
                self.cursor_x -= CHAR_W;
                self.erase_cell(display);
            }
        }
    }

    /// 全消去して先頭行へ。
    fn clear<D>(&mut self, display: &mut D)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let _ = display.clear(Rgb565::BLACK);
        self.reset();
    }

    /// カーソル位置のアンダーラインを指定色で塗る。
    ///
    /// カーソルは常に空セル上にあるため、塗っても文字を壊さない。
    fn fill_cursor<D>(&self, display: &mut D, color: Rgb565)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let _ = Rectangle::new(
            Point::new(self.cursor_x, self.cursor_y + 2),
            Size::new(CHAR_W as u32, 3),
        )
        .into_styled(PrimitiveStyle::with_fill(color))
        .draw(display);
    }
}

/// 現在の入力を POST 送信し、応答を画面に表示してからエディタをリセットする。
async fn send_message<D>(
    editor: &mut Editor,
    display: &mut D,
    style: MonoTextStyle<'_, Rgb565>,
    stack: Stack<'_>,
) where
    D: DrawTarget<Color = Rgb565>,
{
    let mut rx = [0u8; 1024];
    let mut tx = [0u8; 512];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);

    // 応答は ~4KB の画像になるためヒープに確保する。
    let mut response = alloc::vec![0u8; RESPONSE_CAP];

    let result = net::http_post(
        &mut socket,
        config::SERVER_IP,
        config::SERVER_PORT,
        config::RENDER_PATH,
        config::SERVER_HOST,
        editor.message(),
        &mut response,
    )
    .await;

    match result {
        Ok(len) => {
            // 応答本文（1bit 画像）をそのまま画面に転送する。
            let body = net::extract_body(&response[..len]);
            let _ = ui::draw_image_1bpp(display, body, SCREEN_W, SCREEN_H);
        }
        Err(_) => {
            let _ = ui::show_message(display, style, "Send error");
        }
    }

    // 受信内容は上部に出るので、カーソルは最下行へ。
    editor.reset_to_bottom();
}

/// 録音済み PCM を `/stt` に送り、文字起こし結果の画像を表示する。
async fn send_stt<D>(
    editor: &mut Editor,
    display: &mut D,
    style: MonoTextStyle<'_, Rgb565>,
    stack: Stack<'_>,
    pcm: &[u8],
    path: &str,
) where
    D: DrawTarget<Color = Rgb565>,
{
    let _ = ui::show_message(display, style, "Transcribing...");

    let mut rx = [0u8; 1024];
    let mut tx = [0u8; 1024];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);

    let mut response = alloc::vec![0u8; RESPONSE_CAP];

    let result = net::http_post(
        &mut socket,
        config::SERVER_IP,
        config::SERVER_PORT,
        path,
        config::SERVER_HOST,
        pcm,
        &mut response,
    )
    .await;

    match result {
        Ok(len) => {
            let body = net::extract_body(&response[..len]);
            let _ = ui::draw_image_1bpp(display, body, SCREEN_W, SCREEN_H);
        }
        Err(_) => {
            let _ = ui::show_message(display, style, "STT error");
        }
    }

    editor.reset_to_bottom();
}

/// `/speak` から回答音声（生 PCM）を取得し、スピーカーで再生する。
///
/// ギャップレス再生のため、まず全体をバッファ（`dl_buf`, 上限 3 秒）へ
/// ダウンロードしてから、循環 DMA で途切れず再生する。上限超過分は切り捨て。
async fn play_answer(stack: Stack<'_>, i2s_tx: &mut I2sTx<'_, Async>) {
    let mut rx = [0u8; 2048];
    let mut tx = [0u8; 512];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);

    if socket
        .connect((config::SERVER_IP, config::SERVER_PORT))
        .await
        .is_err()
    {
        return;
    }

    let mut req_buf = [0u8; 96];
    let req = net::write_get_request(
        &mut req_buf,
        config::SPEAK_PATH,
        config::SERVER_HOST,
    );
    if socket.write(req).await.is_err() || socket.flush().await.is_err() {
        return;
    }

    // レスポンスを読み、ヘッダ（\r\n\r\n）以降の本文を dl_buf へ貯める。
    let mut byte_len = 0usize;
    {
        let dl = audio::dl_buf();
        let mut buf = [0u8; 512];
        let mut header_done = false;
        let mut m = 0u8; // \r\n\r\n のマッチ状態

        loop {
            let n = match socket.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => break,
            };
            if n == 0 {
                break;
            }

            let mut start = 0;
            if !header_done {
                let mut i = 0;
                while i < n {
                    let b = buf[i];
                    m = match (m, b) {
                        (0, b'\r') => 1,
                        (1, b'\n') => 2,
                        (2, b'\r') => 3,
                        (3, b'\n') => 4,
                        (_, b'\r') => 1,
                        _ => 0,
                    };
                    i += 1;
                    if m == 4 {
                        header_done = true;
                        start = i;
                        break;
                    }
                }
                if !header_done {
                    continue;
                }
            }

            let take = core::cmp::min(dl.len() - byte_len, n - start);
            if take > 0 {
                dl[byte_len..byte_len + take]
                    .copy_from_slice(&buf[start..start + take]);
                byte_len += take;
            }
            if byte_len == dl.len() {
                break; // 上限（3秒）に達したら以降は捨てる
            }
        }
    }

    // ダウンロード完了 → 途切れなく再生。
    audio::play_pcm_gapless(i2s_tx, byte_len);

    // 循環 DMA 後、TX を通常（ワンショット）状態へ戻す。これをしないと
    // 次回の録音（TX 無音でクロック供給）が働かず、音声を拾えなくなる。
    audio::prime(i2s_tx).await;
}

/// キーボード入力ループ。戻らない（端末が動いている間ずっと動作）。
///
/// `stack` が `Some` のときは Enter で入力を POST 送信し、応答を表示する。
/// `None`（オフライン）のときは Enter は改行になる。
/// Fn + W でアクセスポイント一覧を表示する（`controller` を使用）。
/// 表示エラーは無視して継続する（端末を落とさないため）。
pub async fn run_input<D, I>(
    display: &mut D,
    keypad: &mut Tca8418<I>,
    style: MonoTextStyle<'_, Rgb565>,
    stack: Option<Stack<'_>>,
    controller: &mut WifiController<'_>,
    i2s_rx: &mut I2sRx<'_, Async>,
    i2s_tx: &mut I2sTx<'_, Async>,
) where
    D: DrawTarget<Color = Rgb565>,
    I: I2c,
{
    let mut editor = Editor::new();
    let mut shift_down = false;
    let mut fn_down = false;

    // 押しっぱなしのキー（生の row/col）とリピート用カウンタ。
    let mut held: Option<(u8, u8)> = None;
    let mut repeat_countdown: u32 = 0;

    // カーソル点滅の状態。
    /// 点滅の周回数（約 500ms）。
    const BLINK_TICKS: u32 = 25;
    let mut blink_on = true;
    let mut blink_ticks: u32 = 0;
    let mut cursor_shown = false;

    // 録音状態（Fn+Space / Fn+A を押している間だけ録音、離したら送信）。
    let mut recording = false;
    let mut rec_len: usize = 0;
    let mut rec_key: (u8, u8) = (0, 0);
    // 録音を /ask（AIに質問）へ送るか、/stt（文字起こしのみ）へ送るか。
    let mut rec_is_ask = false;

    // 起動時に残っているキーイベントを空読みして誤トリガを防ぐ。
    if let Ok(events) = keypad.events() {
        for _ in events {}
    }

    loop {
        // 入力やカーソル描画の前に、表示中のカーソルを消して土台を綺麗にする。
        if cursor_shown {
            editor.fill_cursor(display, Rgb565::BLACK);
            cursor_shown = false;
        }

        let mut acted = false;
        let mut send_requested = false;
        let mut scan_requested = false;
        let mut ip_requested = false;
        let mut tone_requested = false;
        let mut stop_send = false;

        // 録音中は 1 チャンク（約64ms）取り込む。満杯なら自動停止して送信。
        if recording {
            rec_len = audio::capture_chunk(i2s_rx, i2s_tx, rec_len).await;
            if rec_len >= audio::MAX_SAMPLES {
                recording = false;
                stop_send = true;
            }
        }

        // I2C 読み出しに失敗しても panic せず、次の周回で再試行する。
        if let Ok(events) = keypad.events() {
            for event in events {
                if let Some(key) = event.pressed_keypad() {
                    let (row, col) = keyboard::remap_key(key.row, key.col);

                    match (row, col) {
                        // Shift / Fn（修飾キー: リピート対象外）
                        (2, 1) => shift_down = true,
                        (2, 0) => fn_down = true,

                        // Backspace（Fn 併用で全消去）
                        (0, 13) => {
                            if fn_down {
                                editor.clear(display);
                                held = None;
                            } else {
                                editor.backspace(display);
                                held = Some((key.row, key.col));
                                repeat_countdown = REPEAT_DELAY_TICKS;
                            }
                            acted = true;
                        }

                        // Enter: オンラインなら送信、オフラインなら改行
                        (2, 13) => {
                            if stack.is_some() {
                                send_requested = true;
                            } else {
                                editor.newline();
                                acted = true;
                            }
                            held = None;
                        }

                        // その他の修飾キー (Tab/Ctrl/Opt/Alt) は無視
                        (1, 0) | (3, 0) | (3, 1) | (3, 2) => {}

                        // 通常キー: Fn+W は AP 一覧、それ以外は文字入力
                        _ => {
                            if let Some(base) =
                                keyboard::key_to_char(key.row, key.col)
                            {
                                if fn_down && base == 'w' {
                                    scan_requested = true;
                                    held = None;
                                } else if fn_down && base == 'i' {
                                    ip_requested = true;
                                    held = None;
                                } else if fn_down && base == 'p' {
                                    // Fn+P でスピーカー検証用のテスト音。
                                    tone_requested = true;
                                    held = None;
                                } else if fn_down
                                    && (base == ' ' || base == 'a')
                                {
                                    // Fn+Space=文字起こし / Fn+A=AIに質問。
                                    // 押している間だけ録音する。
                                    if stack.is_some() && !recording {
                                        recording = true;
                                        rec_len = 0;
                                        rec_key = (key.row, key.col);
                                        rec_is_ask = base == 'a';
                                        let msg = if rec_is_ask {
                                            "Ask... (speak)"
                                        } else {
                                            "Recording..."
                                        };
                                        let _ =
                                            ui::show_message(display, style, msg);
                                        cursor_shown = false;
                                    }
                                    held = None;
                                } else if fn_down && base == 'r' {
                                    // Fn+R でソフトリセット（戻らない）。
                                    software_reset();
                                } else {
                                    let ch = if shift_down {
                                        keyboard::shift_char(base)
                                    } else {
                                        base
                                    };
                                    editor.put_char(display, style, ch);
                                    held = Some((key.row, key.col));
                                    repeat_countdown = REPEAT_DELAY_TICKS;
                                    acted = true;
                                }
                            }
                        }
                    }
                }

                if let Some(key) = event.released_keypad() {
                    let (row, col) = keyboard::remap_key(key.row, key.col);

                    // 録音キーを離したら録音停止→送信。
                    if recording && (key.row, key.col) == rec_key {
                        recording = false;
                        stop_send = true;
                    }

                    match (row, col) {
                        (2, 1) => shift_down = false,
                        (2, 0) => fn_down = false,
                        _ => {
                            if held == Some((key.row, key.col)) {
                                held = None;
                            }
                        }
                    }
                }
            }
        }

        // Fn+W による AP 一覧表示（反復子を手放してから await する）。
        if scan_requested {
            let _ = ui::show_message(display, style, "Scanning...");

            match wifi::scan(controller, 6).await {
                Ok(aps) => {
                    let _ = ui::show_ap_list(display, style, &aps);
                }
                Err(_) => {
                    let _ = ui::show_message(display, style, "Scan failed");
                }
            }

            // 一覧は上部に出るので、カーソルは最下行へ。
            editor.reset_to_bottom();
            cursor_shown = false;
            acted = true;
        }

        // Fn+I による IP アドレス表示。
        if ip_requested {
            match stack.and_then(|s| s.config_v4()) {
                Some(cfg) => {
                    let _ = ui::show_ip(display, style, cfg.address, cfg.gateway);
                }
                None => {
                    let _ = ui::show_message(display, style, "No IP");
                }
            }

            // IP 表示は上部に出るので、カーソルは最下行へ。
            editor.reset_to_bottom();
            cursor_shown = false;
            acted = true;
        }

        // Fn+P によるスピーカー検証用テスト音（440Hz, 500ms）。
        if tone_requested {
            let _ = ui::show_message(display, style, "Beep...");
            audio::play_tone(i2s_tx, 440, 500).await;
            editor.reset_to_bottom();
            cursor_shown = false;
            acted = true;
        }

        // Enter による送信（イベントの反復子を手放してから await する）。
        if send_requested {
            if let Some(stack) = stack {
                send_message(&mut editor, display, style, stack).await;
                cursor_shown = false;
                acted = true;
            }
        }

        // 録音キーを離した（または満杯）→ 録音を送信。
        // Fn+A なら /ask（AI回答）、Fn+Space なら /stt（文字起こし）。
        if stop_send {
            if let Some(stack) = stack {
                let pcm = audio::pcm_bytes(rec_len);
                let path = if rec_is_ask {
                    config::ASK_PATH
                } else {
                    config::STT_PATH
                };
                send_stt(&mut editor, display, style, stack, pcm, path).await;

                // Fn+A（AI質問）のときは回答を音声でも読み上げる。
                if rec_is_ask {
                    play_answer(stack, i2s_tx).await;
                }
            }
            rec_len = 0;
            cursor_shown = false;
            acted = true;
        }

        // 押しっぱなしのキーをリピート入力する。
        if let Some((raw_row, raw_col)) = held {
            if repeat_countdown > 0 {
                repeat_countdown -= 1;
            }

            if repeat_countdown == 0 {
                let (row, col) = keyboard::remap_key(raw_row, raw_col);

                if (row, col) == (0, 13) {
                    editor.backspace(display);
                } else if let Some(ch) =
                    keyboard::key_to_char(raw_row, raw_col)
                {
                    let ch = if shift_down {
                        keyboard::shift_char(ch)
                    } else {
                        ch
                    };
                    editor.put_char(display, style, ch);
                }

                repeat_countdown = REPEAT_INTERVAL_TICKS;
                acted = true;
            }
        }

        // 録音中は「Recording...」表示のままにし、カーソル点滅も待ちも省く
        // （capture_chunk のブロッキングがペース源）。少しだけ譲る。
        if recording {
            embassy_time::Timer::after_millis(1).await;
            continue;
        }

        // 入力があった直後はカーソルを点灯状態にして即座に見せる。
        if acted {
            blink_on = true;
            blink_ticks = 0;
        }

        // 点滅の位相を進める。
        blink_ticks += 1;
        if blink_ticks >= BLINK_TICKS {
            blink_ticks = 0;
            blink_on = !blink_on;
        }

        // 点灯位相ならカーソルを描画する。
        if blink_on {
            editor.fill_cursor(display, Rgb565::WHITE);
            cursor_shown = true;
        }

        // executor に処理を譲り、ネットワークタスクを動かし続ける。
        embassy_time::Timer::after_millis(20).await;
    }
}
