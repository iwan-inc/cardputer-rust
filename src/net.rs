//! HTTP リクエストの組み立て・送受信・レスポンスの最小限のパース処理（no_std）。

use embassy_net::{
    Ipv4Address,
    tcp::TcpSocket,
};

/// HTTP GET の失敗理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpError {
    /// TCP 接続に失敗した。
    Connect,
    /// 送信に失敗した。
    Send,
    /// 受信に失敗した。
    Receive,
}

/// `ip:port` に接続して GET を送信し、レスポンス全体を `response` に読み込む。
///
/// 読み込んだバイト数を返す。`response` に収まらない分は切り捨てる。
/// どの段階でも panic せず、失敗理由を `Err` で返す。
pub async fn http_get(
    socket: &mut TcpSocket<'_>,
    ip: Ipv4Address,
    port: u16,
    request: &[u8],
    response: &mut [u8],
) -> Result<usize, HttpError> {
    socket
        .connect((ip, port))
        .await
        .map_err(|_| HttpError::Connect)?;

    socket.write(request).await.map_err(|_| HttpError::Send)?;
    socket.flush().await.map_err(|_| HttpError::Send)?;

    let mut read_buffer = [0u8; 512];
    let mut len = 0;

    loop {
        let n = socket
            .read(&mut read_buffer)
            .await
            .map_err(|_| HttpError::Receive)?;

        if n == 0 {
            break;
        }

        let remaining = response.len() - len;
        let take = core::cmp::min(n, remaining);

        response[len..len + take].copy_from_slice(&read_buffer[..take]);
        len += take;

        if len == response.len() {
            break;
        }
    }

    Ok(len)
}

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
