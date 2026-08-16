# cardputer-rust

M5Stack Cardputer Adv (ESP32-S3) 用の Rust `no_std` プロジェクト。
小さなネットワーク端末を目指して段階的に開発している。

## 現在できること

- ESP32-S3 / `no_std` / embassy (async) で動作
- ST7789 LCD 表示
- TCA8418 キーボード入力（Shift / Enter / Backspace）
- Wi-Fi 接続・DHCP 取得
- HTTP サーバーへ GET し、レスポンス本文を LCD 表示

## セットアップ

秘密情報（Wi-Fi 認証情報）は git 管理外。クローン後に次を実行する。

```sh
cp src/secrets.example.rs src/secrets.rs
# src/secrets.rs を編集して自分の SSID / パスワードを設定
```

接続先サーバーなどそのほかの設定は `src/config.rs` にまとまっている。

## ビルド / 実行

esp ツールチェーン（`rust-toolchain.toml` で `channel = "esp"`）が必要。

```sh
cargo build --release
cargo run --release   # espflash で書き込み + モニタ
```

## 構成

- `src/bin/main.rs` — エントリポイント（ペリフェラル初期化とメイン処理）
- `src/config.rs` — 設定値の一元管理
- `src/secrets.rs` — Wi-Fi 認証情報（git 管理外、`secrets.example.rs` から作成）
- `src/wifi.rs` — Wi-Fi 接続設定
- `src/net.rs` — HTTP リクエスト組み立て・レスポンス解析
- `src/keyboard.rs` — キーボードのキーマップ変換
- `src/ui.rs` — LCD 表示ヘルパ
