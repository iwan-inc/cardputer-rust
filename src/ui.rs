//! LCD 表示まわりの共通ヘルパ。

use embedded_graphics::{
    mono_font::{
        MonoTextStyle,
        ascii::FONT_10X20,
    },
    pixelcolor::Rgb565,
    prelude::*,
    text::Text,
};

/// テキスト描画の基準位置（左上寄り、1行目のベースライン）。
const MESSAGE_ORIGIN: Point = Point::new(20, 70);

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
