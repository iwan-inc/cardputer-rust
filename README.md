# cardputer-rust

M5Stack Cardputer Adv (ESP32-S3) 用の Rust `no_std` プロジェクト。
小さなネットワーク端末を目指して段階的に開発している。

## 現在できること

- ESP32-S3 / `no_std` / embassy (async) で動作
- ST7789 LCD 表示、TCA8418 キーボード入力（リピート / 行またぎ Backspace / 点滅カーソル）
- Wi-Fi 接続（リトライ、失敗時は AP 一覧表示 → オフライン動作）
- 起動時に HTTP GET でメッセージ表示
- **Enter で入力をサーバへ送信し、サーバが描画した画像（日本語対応）を表示**
- サーバのコマンド: `tenki [場所]`（天気）, `time`（時刻）, `help`

### 特殊キー

| キー | 動作 |
|---|---|
| Enter | 入力を送信（オフライン時は改行） |
| Fn + Del | 全消去 |
| Fn + W | アクセスポイント一覧 |
| Fn + I | IP アドレス表示 |
| Fn + Space | 押している間マイク録音 → 文字起こし |
| Fn + A | 押している間マイク録音 → 文字起こし → Claude に質問し回答 |
| Fn + R | 再起動 |

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

## サーバー（server/）

入力の送信先。テキストを 240x135 の 1bit 画像に描画して返すため、日本語も
表示できる。macOS の日本語フォント（ヒラギノ等）と Pillow を使う。

```sh
cd server
python3 -m venv .venv
.venv/bin/pip install -r requirements.txt
.venv/bin/python server.py          # 既定ポート 18080
```

デバイスの接続先は `src/config.rs`（`SERVER_IP` / `SERVER_PORT`）で設定する。
`.venv/` は git 管理外。

エンドポイント:

- `POST /render` — テキスト（コマンド）を画像化して返す
- `POST /stt` — 音声（WAV / 16kHz mono s16le）を faster-whisper で日本語に
  文字起こしして画像で返す（初回はモデルを自動 DL、`WHISPER_MODEL` で切替）

コマンド（`/render` の本文先頭語）: `tenki [場所]` / `time` / `ai [質問]` / `help`。
`ai` は Claude（`claude-opus-4-8`）に問い合わせて日本語で回答する。要
`ANTHROPIC_API_KEY`。設定方法は2通り（環境変数が優先）:

```sh
# 方法A: server/.env に書く（推奨・シェル非依存、git 管理外）
cp server/.env.example server/.env   # 値を実キーに編集
# 方法B: 環境変数
export ANTHROPIC_API_KEY=sk-ant-...
```

## 構成

- `src/bin/main.rs` — エントリポイント（ペリフェラル初期化とメイン処理）
- `src/config.rs` — 設定値の一元管理
- `src/secrets.rs` — Wi-Fi 認証情報（git 管理外、`secrets.example.rs` から作成）
- `src/wifi.rs` — Wi-Fi 接続設定・スキャン
- `src/net.rs` — HTTP リクエスト組み立て・送受信・レスポンス解析
- `src/keyboard.rs` — キーボードのキーマップ変換
- `src/terminal.rs` — キーボード入力ループ（送信・AP一覧・IP・再起動）
- `src/ui.rs` — LCD 表示ヘルパ（テキスト・画像・一覧）
- `server/` — Python の API サーバー（画像描画・コマンド）
