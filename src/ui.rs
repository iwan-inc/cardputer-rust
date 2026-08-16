//! LCD 表示まわりの共通ヘルパ。

use core::fmt::Write;

use embedded_graphics::{
    mono_font::{
        MonoTextStyle,
        ascii::FONT_10X20,
    },
    pixelcolor::Rgb565,
    prelude::*,
    text::Text,
};
use esp_radio::wifi::ap::AccessPointInfo;

/// テキスト描画の基準位置（左上寄り、1行目のベースライン）。
const MESSAGE_ORIGIN: Point = Point::new(20, 70);

// AP 一覧のレイアウト
const LIST_LEFT: i32 = 10;
const LIST_TOP: i32 = 20;
const LIST_LINE_H: i32 = 20;
/// ヘッダの下に並べる AP の最大数（画面 6 行 − ヘッダ 1 行）。
const LIST_MAX_ENTRIES: usize = 5;

/// 1 行分の文字列を組み立てるための固定長バッファ（`core::fmt::Write`）。
struct LineBuf {
    buf: [u8; 48],
    len: usize,
}

impl LineBuf {
    fn new() -> Self {
        Self {
            buf: [0u8; 48],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl Write for LineBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let n = core::cmp::min(bytes.len(), self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}

/// 端末の標準テキストスタイル（白・FONT_10X20）。
pub fn text_style() -> MonoTextStyle<'static, Rgb565> {
    MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE)
}

/// 画面を黒でクリアし、1 行メッセージを表示する（状態・エラー表示用）。
pub fn show_message<D>(
    display: &mut D,
    style: MonoTextStyle<'_, Rgb565>,
    text: &str,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    display.clear(Rgb565::BLACK)?;
    Text::new(text, MESSAGE_ORIGIN, style).draw(display)?;
    Ok(())
}

/// アクセスポイント一覧を表示する（ヘッダ + 最大 5 件、SSID/RSSI/ch）。
pub fn show_ap_list<D>(
    display: &mut D,
    style: MonoTextStyle<'_, Rgb565>,
    aps: &[AccessPointInfo],
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    display.clear(Rgb565::BLACK)?;

    let mut header = LineBuf::new();
    let _ = write!(header, "Wi-Fi APs: {}", aps.len());
    Text::new(header.as_str(), Point::new(LIST_LEFT, LIST_TOP), style)
        .draw(display)?;

    for (i, ap) in aps.iter().take(LIST_MAX_ENTRIES).enumerate() {
        let mut line = LineBuf::new();

        // SSID は先頭 12 文字まで（UTF-8 境界を壊さないよう char 単位で）。
        for c in ap.ssid.as_str().chars().take(12) {
            let _ = write!(line, "{}", c);
        }
        let _ = write!(line, " {} ch{}", ap.signal_strength, ap.channel);

        let y = LIST_TOP + (i as i32 + 1) * LIST_LINE_H;
        Text::new(line.as_str(), Point::new(LIST_LEFT, y), style)
            .draw(display)?;
    }

    Ok(())
}
