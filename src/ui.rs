//! LCD 表示まわりの共通ヘルパ。

use embedded_graphics::{
    mono_font::{
        MonoTextStyle,
        ascii::FONT_10X20,
    },
    pixelcolor::Rgb565,
    prelude::RgbColor,
};

/// 端末の標準テキストスタイル（白・FONT_10X20）。
pub fn text_style() -> MonoTextStyle<'static, Rgb565> {
    MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE)
}
