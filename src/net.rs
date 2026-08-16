//! HTTP リクエストの組み立てとレスポンスの最小限のパース処理（純粋・no_std）。

/// `buf` に `GET` リクエストを組み立て、書き込んだ範囲のスライスを返す。
///
/// `Connection: close` を付与する。`buf` に収まらない場合は入る分だけ
/// 書き込む（末尾が欠ける可能性がある点に注意）。
pub fn write_get_request<'a>(
    buf: &'a mut [u8],
    path: &str,
    host: &str,
) -> &'a [u8] {
    let parts: [&[u8]; 5] = [
        b"GET ",
        path.as_bytes(),
        b" HTTP/1.1\r\nHost: ",
        host.as_bytes(),
        b"\r\nConnection: close\r\n\r\n",
    ];

    let mut len = 0;

    for part in parts {
        let n = core::cmp::min(part.len(), buf.len() - len);
        buf[len..len + n].copy_from_slice(&part[..n]);
        len += n;
    }

    &buf[..len]
}

/// HTTP レスポンス全体から、ヘッダ終端 `\r\n\r\n` 以降の本文を切り出す。
///
/// 区切りが見つからない場合は全体をそのまま返す（本文開始 = 0）。
pub fn extract_body(response: &[u8]) -> &[u8] {
    let mut body_start = 0;

    for i in 0..response.len().saturating_sub(3) {
        if &response[i..i + 4] == b"\r\n\r\n" {
            body_start = i + 4;
            break;
        }
    }

    &response[body_start..]
}
