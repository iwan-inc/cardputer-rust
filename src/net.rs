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

    write_all(socket, request).await?;
    socket.flush().await.map_err(|_| HttpError::Send)?;

    read_response(socket, response).await
}

/// `ip:port` に接続して POST を送信し、レスポンス全体を `response` に読み込む。
///
/// `body` を `text/plain` として送る。読み込んだバイト数を返す。
/// どの段階でも panic せず、失敗理由を `Err` で返す。
pub async fn http_post(
    socket: &mut TcpSocket<'_>,
    ip: Ipv4Address,
    port: u16,
    path: &str,
    host: &str,
    body: &[u8],
    response: &mut [u8],
) -> Result<usize, HttpError> {
    socket
        .connect((ip, port))
        .await
        .map_err(|_| HttpError::Connect)?;

    let mut header = [0u8; 192];
    let header = write_post_header(&mut header, path, host, body.len());

    write_all(socket, header).await?;
    write_all(socket, body).await?;
    socket.flush().await.map_err(|_| HttpError::Send)?;

    read_response(socket, response).await
}

/// ソケットへ `data` を全部書き込む（部分書き込みに対応）。
async fn write_all(
    socket: &mut TcpSocket<'_>,
    mut data: &[u8],
) -> Result<(), HttpError> {
    while !data.is_empty() {
        let n = socket.write(data).await.map_err(|_| HttpError::Send)?;

        if n == 0 {
            return Err(HttpError::Send);
        }

        data = &data[n..];
    }

    Ok(())
}

/// レスポンスを `response` へ読み込み、読み込んだバイト数を返す。
async fn read_response(
    socket: &mut TcpSocket<'_>,
    response: &mut [u8],
) -> Result<usize, HttpError> {
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

/// `buf` に `POST` リクエストのヘッダ（本文は含まない）を組み立てる。
///
/// `Content-Type: text/plain` と `Content-Length` を付与する。
/// 本文は呼び出し側が別途送る。
pub fn write_post_header<'a>(
    buf: &'a mut [u8],
    path: &str,
    host: &str,
    content_length: usize,
) -> &'a [u8] {
    let mut len = 0;

    push(buf, &mut len, b"POST ");
    push(buf, &mut len, path.as_bytes());
    push(buf, &mut len, b" HTTP/1.1\r\nHost: ");
    push(buf, &mut len, host.as_bytes());
    push(
        buf,
        &mut len,
        b"\r\nContent-Type: text/plain\r\nContent-Length: ",
    );
    push_usize(buf, &mut len, content_length);
    push(buf, &mut len, b"\r\nConnection: close\r\n\r\n");

    &buf[..len]
}

/// `buf[*len..]` へ `src` を追記する（あふれた分は切り捨て）。
fn push(buf: &mut [u8], len: &mut usize, src: &[u8]) {
    let n = core::cmp::min(src.len(), buf.len() - *len);
    buf[*len..*len + n].copy_from_slice(&src[..n]);
    *len += n;
}

/// `buf[*len..]` へ `n` を 10 進数で追記する。
fn push_usize(buf: &mut [u8], len: &mut usize, mut n: usize) {
    let mut tmp = [0u8; 20];
    let mut i = tmp.len();

    if n == 0 {
        i -= 1;
        tmp[i] = b'0';
    } else {
        while n > 0 {
            i -= 1;
            tmp[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
    }

    push(buf, len, &tmp[i..]);
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
