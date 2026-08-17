#!/usr/bin/env python3
"""Cardputer 用の簡易 API サーバー。

- GET  : カレントディレクトリのファイルを配信（/hello.txt など）
- POST /render : 本文をコマンドとして解釈し、結果を 240x135 の 1bit モノクロ
                 画像に描画して返す（日本語対応）。デバイスはこの画素を
                 そのまま LCD に転送する。
- POST /stt    : 音声（WAV か 16kHz/mono/s16le の生 PCM）を受け取り、
                 faster-whisper で日本語に文字起こしして画像で返す。
- POST /ask    : 音声を文字起こしし、その内容を Claude に質問して
                 回答（日本語）を画像で返す（要 ANTHROPIC_API_KEY）。
                 回答は macOS say で音声合成して保持する。
- GET  /speak  : 直近の回答音声（16kHz/mono/s16le の生 PCM）を返す。
                 デバイスがストリーミング再生する。
- POST (その他) : 本文をコンソール表示し、確認メッセージを返す。

文字起こしモデルは環境変数 WHISPER_MODEL で切替（既定 small）。
初回の /stt 呼び出し時にモデルを自動ダウンロードする。

ai/ask には ANTHROPIC_API_KEY が必要。環境変数か、server.py と同じ
ディレクトリの .env（KEY=VALUE 形式、git 管理外）で設定する。
環境変数が優先。

セットアップと起動（Pillow を入れた venv で起動する）:
    cd server
    python3 -m venv .venv
    .venv/bin/pip install -r requirements.txt
    .venv/bin/python server.py            # 既定ポート 18080
    .venv/bin/python server.py 9000       # ポート指定

コマンド（/render の本文）:
    tenki [場所(英字)]   天気（Open-Meteo, APIキー不要）。省略時 Tokyo
    time                 現在時刻（サーバのローカル時刻）
    ai [質問(英字)]      Claude に質問（要 ANTHROPIC_API_KEY）。日本語で回答
    help                 コマンド一覧
    それ以外              「受信: ...」をエコー
"""
import io
import json
import os
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request
import wave
from datetime import datetime
from http.server import HTTPServer, SimpleHTTPRequestHandler

import numpy as np
from PIL import Image, ImageDraw, ImageFont

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18080


def _load_dotenv():
    """server.py と同じディレクトリの .env を読み、未設定の環境変数だけ設定する。

    シェルの環境変数が優先（既にあれば上書きしない）。`KEY=VALUE` 形式、
    先頭 # はコメント、値の前後のクォートは除去する。これにより
    `~/.zshrc` の読み込み有無に依存せず ANTHROPIC_API_KEY を渡せる。
    """
    path = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".env")
    if not os.path.exists(path):
        return
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                line = line.strip()
                if not line or line.startswith("#") or "=" not in line:
                    continue
                key, _, val = line.partition("=")
                key = key.strip()
                val = val.strip().strip('"').strip("'")
                if key and key not in os.environ:
                    os.environ[key] = val
    except OSError:
        pass


_load_dotenv()

# LCD の解像度（デバイス側と一致させること）。
WIDTH, HEIGHT = 240, 135
FONT_SIZE = 16
LINE_HEIGHT = FONT_SIZE + 4

# 日本語を含むフォント候補（見つかった最初のものを使う）。
FONT_CANDIDATES = [
    ("/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc", 0),
    ("/System/Library/Fonts/Supplemental/Arial Unicode.ttf", 0),
    ("/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
]


def load_font(size):
    for path, index in FONT_CANDIDATES:
        try:
            return ImageFont.truetype(path, size=size, index=index)
        except OSError:
            continue
    return ImageFont.load_default()  # 最後の手段（英数字のみ）


FONT = load_font(FONT_SIZE)


def render_text_1bpp(text):
    """テキストを WIDTH x HEIGHT の 1bit 画像に描き、生バイト列を返す。

    折り返しは文字単位（日本語向け）。改行 (\\n) も反映する。
    戻り値は Pillow mode "1" の tobytes()（MSB 先頭、行は 1 バイト境界に
    パディング）。WIDTH=240 は 8 の倍数なので 30 バイト/行 * 135 = 4050 バイト。
    """
    img = Image.new("1", (WIDTH, HEIGHT), 0)  # 0 = 黒
    draw = ImageDraw.Draw(img)

    x = 0
    y = 0
    for ch in text:
        if ch == "\r":
            continue
        if ch == "\n":
            x = 0
            y += LINE_HEIGHT
            continue

        w = draw.textlength(ch, font=FONT)
        if x + w > WIDTH:
            x = 0
            y += LINE_HEIGHT
        if y + LINE_HEIGHT > HEIGHT:
            break  # 画面に収まらない分は捨てる

        draw.text((x, y), ch, fill=255, font=FONT)  # 255 = 白
        x += w

    return img.tobytes()


# ---- コマンド ----

# WMO 天気コード → 日本語
WEATHER_CODES = {
    0: "快晴", 1: "晴れ", 2: "薄曇り", 3: "曇り",
    45: "霧", 48: "霧氷",
    51: "霧雨", 53: "霧雨", 55: "強い霧雨",
    56: "着氷性の霧雨", 57: "着氷性の霧雨",
    61: "小雨", 63: "雨", 65: "強い雨",
    66: "着氷性の雨", 67: "着氷性の雨",
    71: "小雪", 73: "雪", 75: "大雪", 77: "霧雪",
    80: "にわか雨", 81: "にわか雨", 82: "激しいにわか雨",
    85: "にわか雪", 86: "強いにわか雪",
    95: "雷雨", 96: "雷雨(雹)", 99: "雷雨(雹)",
}

WEEKDAYS_JA = ["月", "火", "水", "木", "金", "土", "日"]


def _get_json(url):
    with urllib.request.urlopen(url, timeout=8) as r:
        return json.load(r)


def cmd_tenki(arg):
    """`tenki [場所(英字)]` で天気を返す。省略時は Tokyo。"""
    place = arg.strip() or "Tokyo"

    geo = _get_json(
        "https://geocoding-api.open-meteo.com/v1/search?"
        + urllib.parse.urlencode({"name": place, "count": 1, "language": "ja"})
    )
    results = geo.get("results")
    if not results:
        return f"見つかりません: {place}"

    loc = results[0]
    name = loc.get("name", place)

    fc = _get_json(
        "https://api.open-meteo.com/v1/forecast?"
        + urllib.parse.urlencode(
            {
                "latitude": loc["latitude"],
                "longitude": loc["longitude"],
                "current": "temperature_2m,weather_code",
                "daily": "weather_code,temperature_2m_max,temperature_2m_min",
                "timezone": "auto",
                "forecast_days": 1,
            }
        )
    )

    cur = fc["current"]
    daily = fc["daily"]
    now_desc = WEATHER_CODES.get(cur["weather_code"], "?")
    day_desc = WEATHER_CODES.get(daily["weather_code"][0], "?")
    temp = cur["temperature_2m"]
    tmax = daily["temperature_2m_max"][0]
    tmin = daily["temperature_2m_min"][0]

    return (
        f"{name}の天気\n"
        f"現在: {now_desc} {temp}℃\n"
        f"今日: {day_desc}\n"
        f"最高{tmax}℃ 最低{tmin}℃"
    )


def cmd_time(arg):
    """現在時刻（サーバのローカル時刻）を返す。"""
    now = datetime.now()
    wd = WEEKDAYS_JA[now.weekday()]
    return now.strftime("現在時刻\n%Y-%m-%d (") + wd + now.strftime(")\n%H:%M:%S")


_ANTHROPIC = None


def _get_anthropic():
    """Anthropic クライアントを遅延生成する（ANTHROPIC_API_KEY を読む）。"""
    global _ANTHROPIC
    if _ANTHROPIC is None:
        import anthropic

        _ANTHROPIC = anthropic.Anthropic()
    return _ANTHROPIC


def cmd_ai(arg):
    """`ai <質問>` で Claude に質問し、日本語の短い回答を返す。"""
    question = arg.strip()
    if not question:
        return "使い方: ai <質問(英字)>"

    if not os.environ.get("ANTHROPIC_API_KEY"):
        return "APIキー未設定\nserver/.env か環境変数\nで設定してください"

    now = datetime.now()
    wd = WEEKDAYS_JA[now.weekday()]
    today = now.strftime(f"%Y年%m月%d日（{wd}）%H:%M")

    system = (
        f"現在の日時は {today}（サーバのローカル時刻）。"
        "日付や時刻に関する質問にはこれを基準に答える。"
        "あなたはカードサイズの小型端末の音声アシスタント。"
        "天気を聞かれたら get_weather ツールを使って実際の天気で答える"
        "（location は英語/ローマ字で渡す。例: 大阪→Osaka, 札幌→Sapporo。"
        "省略時は Tokyo）。"
        "回答は必ず日本語で、2〜3文・全体でおよそ60文字以内に収める"
        "（音声で読み上げるため簡潔に。必要なら一言の補足はよいが冗長にしない）。"
        "前置き・言い訳・繰り返しはしない。最終的な答えだけを書く。"
        "\n出力は必ず次の2行構成にする:"
        "\n1行目: 表示用の回答（通常の漢字かな交じり）。"
        "\n2行目: 『よみ:』に続けて、1行目の全文の読みをひらがなで書く"
        "（音声合成用。漢字・数字・記号もすべて読み方をひらがなに。"
        "例: 富士山→ふじさん、3776m→さんぜんななひゃくななじゅうろくめーとる、"
        "17日→じゅうしちにち、28℃→にじゅうはちど）。"
    )
    tools = [
        {
            "name": "get_weather",
            "description": (
                "指定した場所の現在の天気と今日の予報（気温・天気）を取得する。"
                "天気について聞かれたら必ずこれを使う。"
            ),
            "input_schema": {
                "type": "object",
                "properties": {
                    "location": {
                        "type": "string",
                        "description": (
                            "地名を英語/ローマ字で。例: Osaka, Tokyo, Sapporo。"
                            "（ジオコーディングが日本語漢字では一致しないため）"
                        ),
                    }
                },
                "required": ["location"],
            },
        }
    ]

    client = _get_anthropic()
    messages = [{"role": "user", "content": question}]
    # ツール利用ループ（get_weather を最大数回まで実行）。ストリーミングで
    # 最初のトークン時刻(claude_first)と完了時刻(claude_end)を計測する。
    global _LAST_TIMING
    t_first = None
    for _ in range(4):
        with client.messages.stream(
            model="claude-opus-4-8",
            max_tokens=512,
            system=system,
            tools=tools,
            messages=messages,
        ) as stream:
            for event in stream:
                if (
                    t_first is None
                    and event.type == "content_block_delta"
                    and getattr(event.delta, "type", None) == "text_delta"
                ):
                    t_first = time.perf_counter()
            resp = stream.get_final_message()
        if resp.stop_reason != "tool_use":
            break
        messages.append({"role": "assistant", "content": resp.content})
        results = []
        for block in resp.content:
            if block.type == "tool_use" and block.name == "get_weather":
                loc = (block.input or {}).get("location", "Tokyo")
                print(f"  tool get_weather({loc!r})", flush=True)
                results.append({
                    "type": "tool_result",
                    "tool_use_id": block.id,
                    "content": cmd_tenki(loc),
                })
        messages.append({"role": "user", "content": results})

    _LAST_TIMING = {"claude_first": t_first, "claude_end": time.perf_counter()}

    text = "".join(
        block.text for block in resp.content if block.type == "text"
    ).strip()

    # 表示用(漢字)と読み上げ用(かな)を分離する。『よみ:』以降が読み。
    global _LAST_READING
    display = text
    reading = text
    marker = text.find("よみ:")
    if marker == -1:
        marker = text.find("よみ：")  # 全角コロンも許容
    if marker != -1:
        display = text[:marker].strip()
        reading = text[marker:].split(":", 1)[-1]
        reading = reading.split("：", 1)[-1].strip()
    _LAST_READING = reading or display
    return display or "(回答なし)"


def cmd_help(arg):
    return "コマンド:\ntenki [場所]\ntime\nai [質問]\nhelp"


# コマンド名(小文字) -> 関数(引数文字列 -> 応答文字列)
COMMANDS = {
    "tenki": cmd_tenki,
    "time": cmd_time,
    "ai": cmd_ai,
    "help": cmd_help,
}


def handle_command(text):
    """入力テキストをコマンドとして処理する。未知なら echo。"""
    stripped = text.strip()
    head, _, arg = stripped.partition(" ")
    handler = COMMANDS.get(head.lower())

    if handler is None:
        return f"受信: {text}"

    try:
        return handler(arg)
    except Exception as e:
        return f"エラー: {e}"


# ---- 音声文字起こし（STT: faster-whisper）----

_WHISPER_MODEL = None


def _get_whisper():
    """Whisper モデルを遅延ロードする（初回のみモデルを DL）。

    モデルは環境変数 WHISPER_MODEL で切替（既定 small）。tiny/base/small/medium。
    """
    global _WHISPER_MODEL
    if _WHISPER_MODEL is None:
        from faster_whisper import WhisperModel

        name = os.environ.get("WHISPER_MODEL", "small")
        print(f"Loading Whisper model: {name} ...", flush=True)
        _WHISPER_MODEL = WhisperModel(name, device="cpu", compute_type="int8")
        print("Whisper model loaded.", flush=True)
    return _WHISPER_MODEL


def _decode_audio(data):
    """WAV（優先）または 16kHz/mono/s16le の生 PCM を 16kHz float32 に変換。"""
    sr = 16000
    try:
        with wave.open(io.BytesIO(data), "rb") as w:
            sr = w.getframerate()
            ch = w.getnchannels()
            sw = w.getsampwidth()
            frames = w.readframes(w.getnframes())
        if sw != 2:
            raise ValueError("16-bit PCM のみ対応")
        audio = np.frombuffer(frames, dtype=np.int16).astype(np.float32) / 32768.0
        if ch > 1:
            audio = audio.reshape(-1, ch).mean(axis=1)
    except (wave.Error, EOFError, ValueError):
        # WAV でなければ 16kHz mono s16le の生 PCM とみなす。
        audio = np.frombuffer(data, dtype=np.int16).astype(np.float32) / 32768.0

    # 16kHz でなければ線形補間でリサンプル。
    if sr != 16000 and len(audio) > 1:
        n = int(round(len(audio) * 16000 / sr))
        audio = np.interp(
            np.linspace(0, len(audio), n, endpoint=False),
            np.arange(len(audio)),
            audio,
        ).astype(np.float32)

    return audio


def _save_debug_wav(data):
    """デバッグ用に、受信音声を last_stt.wav（16kHz mono s16le）で保存する。"""
    try:
        audio = _decode_audio(data)
        pcm = (np.clip(audio, -1.0, 1.0) * 32767).astype(np.int16).tobytes()
        with wave.open("last_stt.wav", "wb") as w:
            w.setnchannels(1)
            w.setsampwidth(2)
            w.setframerate(16000)
            w.writeframes(pcm)
        print(f"  saved last_stt.wav ({len(pcm)} bytes)", flush=True)
    except Exception as e:
        print(f"  save failed: {e}", flush=True)


def transcribe(data):
    """音声バイト列を日本語で文字起こしする。"""
    audio = _decode_audio(data)
    if len(audio) < 1600:  # 0.1 秒未満
        return "（音声が短すぎます）"

    model = _get_whisper()
    segments, _info = model.transcribe(audio, language="ja", vad_filter=True)
    text = "".join(seg.text for seg in segments).strip()
    return text or "（認識できませんでした）"


# ---- 音声合成（TTS: macOS say）----

# 直近の回答音声（16kHz/mono/s16le の生 PCM）。/speak で配信する。
_LAST_TTS = b""

# 直近の回答の読み上げ用テキスト（cmd_ai がひらがな読みをセット）。
_LAST_READING = ""

# 直近の Claude 呼び出しのタイムスタンプ（perf_counter）。/ask が内訳表示に使う。
_LAST_TIMING = {}

# デバイスの再生バッファ上限（16kHz/s16le で 6 秒）。超過分は切れる。
DEVICE_PLAY_CAP = 16000 * 6 * 2


def _synthesize_tts(text):
    """text を音声合成し 16kHz/mono/s16le の生 PCM を返す（macOS say）。"""
    text = text.strip()
    if not text:
        return b""
    try:
        with tempfile.TemporaryDirectory() as d:
            aiff = os.path.join(d, "s.aiff")
            wavp = os.path.join(d, "s.wav")
            # -r で話速を少し上げ、6秒バッファに収まる文字数を増やす
            # （既定より速いが自然に聞こえる範囲）。
            subprocess.run(
                ["say", "-v", "Kyoko", "-r", "200", "-o", aiff, text],
                check=True,
                timeout=30,
            )
            subprocess.run(
                ["afconvert", "-f", "WAVE", "-d", "LEI16@16000", "-c", "1",
                 aiff, wavp],
                check=True,
                timeout=30,
            )
            with wave.open(wavp, "rb") as w:
                return w.readframes(w.getnframes())
    except Exception as e:
        print(f"  tts failed: {e}", flush=True)
        return b""


def _print_ask_latency(t_upload, t_stt, timing, t_tts):
    """/ask の各段階のレイテンシ内訳を upload_end 基準の相対秒で表示する。"""
    base = t_upload
    rows = [("upload_end", t_upload)]
    rows.append(("stt_end", t_stt))
    cf = timing.get("claude_first")
    if cf is not None:
        rows.append(("claude_first", cf))
    ce = timing.get("claude_end")
    if ce is not None:
        rows.append(("claude_end", ce))
    rows.append(("tts_end", t_tts))
    print("  --- latency (server, from upload_end) ---", flush=True)
    prev = base
    for name, t in rows:
        rel = t - base
        step = t - prev
        print(f"    {name:<13} +{rel:5.2f}s  (Δ{step:5.2f}s)", flush=True)
        prev = t
    print(
        "    (device adds: record→upload before this, and "
        "/speak download→play after)",
        flush=True,
    )


class Handler(SimpleHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/speak":
            # 直近の回答音声（生 PCM）を配信する。
            payload = _LAST_TTS
            print(f"[GET /speak] {len(payload)} bytes", flush=True)
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(payload)
            return
        super().do_GET()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length) if length else b""

        if self.path == "/stt":
            # 音声（WAV か生 PCM）を文字起こしして画像で返す。
            print(f"[POST /stt] {len(body)} bytes", flush=True)
            _save_debug_wav(body)
            text = transcribe(body)
            print(f"  -> {text!r}", flush=True)
            payload = render_text_1bpp(f"認識: {text}")
            content_type = "application/octet-stream"
        elif self.path == "/ask":
            # 音声を文字起こしし、その内容を Claude に質問して回答を返す。
            # 各段階の所要時間を計測し、最後に内訳を表示する。
            t_upload = time.perf_counter()  # 本文受信完了 = upload_end
            print(f"[POST /ask] {len(body)} bytes", flush=True)
            _save_debug_wav(body)
            question = transcribe(body)
            t_stt = time.perf_counter()
            print(f"  STT -> {question!r}", flush=True)
            answer = cmd_ai(question)  # 副作用で _LAST_READING, _LAST_TIMING をセット
            print(f"  AI  -> {answer!r}", flush=True)
            print(f"  YOMI-> {_LAST_READING!r}", flush=True)
            # 読み上げ用（ひらがな）を音声合成して保持。漢字誤読を防ぐため
            # 表示用の漢字ではなく _LAST_READING を合成する。
            global _LAST_TTS
            _LAST_TTS = _synthesize_tts(_LAST_READING or answer)
            t_tts = time.perf_counter()
            secs = len(_LAST_TTS) / 2 / 16000
            print(f"  TTS -> {len(_LAST_TTS)} bytes ({secs:.1f}s)", flush=True)
            if len(_LAST_TTS) > DEVICE_PLAY_CAP:
                print(
                    f"  WARN: TTS {secs:.1f}s > device cap 6.0s; tail will be"
                    " cut. Answer/reading too long.",
                    flush=True,
                )
            _print_ask_latency(t_upload, t_stt, _LAST_TIMING, t_tts)
            payload = render_text_1bpp(answer)
            content_type = "application/octet-stream"
        elif self.path == "/render":
            text = body.decode("utf-8", errors="replace")
            print(f"[POST /render] {text!r}", flush=True)
            content = handle_command(text)
            print(f"  -> {content!r}", flush=True)
            payload = render_text_1bpp(content)
            content_type = "application/octet-stream"
        else:
            text = body.decode("utf-8", errors="replace")
            print(f"[POST {self.path}] {text!r}", flush=True)
            payload = f"[server] {text}".encode("utf-8")
            content_type = "text/plain; charset=utf-8"

        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(payload)


if __name__ == "__main__":
    print(f"Serving on 0.0.0.0:{PORT} (GET: files, POST /render: image)")
    print(f"Font: {FONT.path if hasattr(FONT, 'path') else 'default'}")
    # Whisper を起動時にロードしておき、初回 /ask のモデルロード待ち
    # （STT に数秒上乗せ）を無くす。
    try:
        _get_whisper()
    except Exception as e:
        print(f"Whisper preload skipped: {e}", flush=True)
    # 0.0.0.0 で LAN 上のデバイスから到達可能にする。
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
