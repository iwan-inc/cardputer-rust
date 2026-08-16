#!/usr/bin/env python3
"""Cardputer 用の簡易 API サーバー。

- GET  : カレントディレクトリのファイルを配信（/hello.txt など）
- POST /render : 本文をコマンドとして解釈し、結果を 240x135 の 1bit モノクロ
                 画像に描画して返す（日本語対応）。デバイスはこの画素を
                 そのまま LCD に転送する。
- POST (その他) : 本文をコンソール表示し、確認メッセージを返す。

セットアップと起動（Pillow を入れた venv で起動する）:
    cd server
    python3 -m venv .venv
    .venv/bin/pip install -r requirements.txt
    .venv/bin/python server.py            # 既定ポート 18080
    .venv/bin/python server.py 9000       # ポート指定

コマンド（/render の本文）:
    tenki [場所(英字)]   天気（Open-Meteo, APIキー不要）。省略時 Tokyo
    time                 現在時刻（サーバのローカル時刻）
    help                 コマンド一覧
    それ以外              「受信: ...」をエコー
"""
import json
import sys
import urllib.parse
import urllib.request
from datetime import datetime
from http.server import HTTPServer, SimpleHTTPRequestHandler

from PIL import Image, ImageDraw, ImageFont

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18080

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


def cmd_help(arg):
    return "コマンド:\ntenki [場所]\ntime\nhelp"


# コマンド名(小文字) -> 関数(引数文字列 -> 応答文字列)
COMMANDS = {
    "tenki": cmd_tenki,
    "time": cmd_time,
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


class Handler(SimpleHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(length) if length else b""
        text = body.decode("utf-8", errors="replace")
        print(f"[POST {self.path}] {text!r}", flush=True)

        if self.path == "/render":
            content = handle_command(text)
            print(f"  -> {content!r}", flush=True)
            payload = render_text_1bpp(content)
            content_type = "application/octet-stream"
        else:
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
    # 0.0.0.0 で LAN 上のデバイスから到達可能にする。
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
