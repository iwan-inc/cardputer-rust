//! ES8311 オーディオコーデックの最小ドライバ（マイク/ADC 入力のみ）。
//!
//! Cardputer Adv のマイクは ES8311 経由（I2C 制御 + 標準 I2S）。
//! MCLK ピンは無く、ES8311 は BCLK を内部 MCLK として使う設定にする。
//!
//! 初期化列は esp-bsp / ESP-IDF の es8311 ドライバ（16kHz マイク構成）を参考に
//! した best-effort な値。実機での調整（特にゲイン 0x16 / 0x17）が必要になりうる。

use embedded_hal::i2c::I2c;

/// ES8311 の I2C アドレス（7bit）。
const ADDR: u8 = 0x18;

pub struct Es8311<I2C> {
    i2c: I2C,
}

impl<I2C: I2c> Es8311<I2C> {
    pub fn new(i2c: I2C) -> Self {
        Self { i2c }
    }

    /// 内部の I2C を取り出す（同じバスをキーボードと共有するため）。
    pub fn into_inner(self) -> I2C {
        self.i2c
    }

    fn write(&mut self, reg: u8, val: u8) -> Result<(), I2C::Error> {
        self.i2c.write(ADDR, &[reg, val])
    }

    /// マイク（ADC）入力を 16kHz / 16bit / BCLK 駆動で初期化する。
    pub fn init_mic(&mut self) -> Result<(), I2C::Error> {
        // リセット
        self.write(0x00, 0x1F)?;
        spin_delay();
        self.write(0x00, 0x00)?;
        self.write(0x00, 0x80)?; // パワーオン

        // クロックマネージャ。
        // BCLK を MCLK として使う（reg01 BIT7）。I2S は 32bit フレーム
        // （channel=32bit, 2ch, 16kHz）= BCLK 1.024MHz にするので、
        // ES8311 係数表の mclk=1024000 / rate=16000 行を使う。
        // {pre_div=1, pre_multi=4, adc_div=1, dac_div=1, fs_mode=0,
        //  lrck=0x00FF, bclk_div=4, adc_osr=0x10, dac_osr=0x10}
        self.write(0x01, 0xBF)?;
        self.write(0x02, 0x10)?; // pre_div=1, pre_multi=4
        self.write(0x03, 0x10)?; // fs_mode=0, adc_osr=0x10
        self.write(0x04, 0x10)?; // dac_osr=0x10
        self.write(0x05, 0x00)?; // adc_div=1, dac_div=1
        self.write(0x06, 0x03)?; // bclk_div=4
        self.write(0x07, 0x00)?; // lrck_h
        self.write(0x08, 0xFF)?; // lrck_l

        // フォーマット（16bit）
        self.write(0x09, 0x0C)?;
        self.write(0x0A, 0x0C)?;

        // ADC / マイク経路の電源・ゲイン
        self.write(0x0D, 0x01)?; // アナログ電源 ON
        self.write(0x0E, 0x02)?; // PGA + ADC モジュレータ
        self.write(0x14, 0x1A)?; // アナログマイク選択 + PGA 最大
        self.write(0x15, 0x00)?;
        self.write(0x16, 0x06)?; // マイクゲイン（要調整）
        self.write(0x17, 0xC8)?; // ADC デジタルゲイン
        self.write(0x1B, 0x0A)?;
        self.write(0x1C, 0x6A)?; // ADC EQ バイパス + DC オフセット除去

        self.write(0x00, 0x80)?; // 再度パワーオン確定
        Ok(())
    }
}

/// I2C レジスタ設定間の粗いディレイ（リセット待ち用）。
fn spin_delay() {
    for _ in 0..200_000 {
        core::hint::spin_loop();
    }
}
