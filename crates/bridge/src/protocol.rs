//! bridge の wire protocol。
//!
//! jvlink-bridge.exe が stdout に書き、ホスト側が読む。
//! フォーマット: `[u32 LE len][u8 tag][payload]` (len = tag + payload の長さ)。
//!
//! コマンドは argv で渡す (一方向ストリームのみ)。

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// JVOpen 成功。read_count=対象ファイル数, download_count=要DL数,
    /// last_file_timestamp=最終ファイル提供時刻 (差分取得の起点)
    OpenOk {
        read_count: i32,
        download_count: i32,
        last_file_timestamp: String,
    },
    /// ダウンロード進捗 (JVStatus のポーリング)
    Progress { done: i32, total: i32 },
    /// 新しいファイルの読み出し開始。
    /// timestamp は m_CurrentFileTimestamp (そのファイルの提供時刻)。
    FileBegin { name: String, timestamp: String },
    /// raw record 1 件 (JVGets が返したバイト列そのまま)
    Record(Vec<u8>),
    /// ファイル終端 (JVGets が -1 を返した)
    FileEnd { name: String },
    /// 全ファイル読了 (JVGets が 0 を返した)
    Done,
    /// JV-Link エラー (負のリターンコード)
    Error { code: i32, message: String },
}

const T_OPEN_OK: u8 = 0x01;
const T_PROGRESS: u8 = 0x02;
const T_FILE_BEGIN: u8 = 0x03;
const T_RECORD: u8 = 0x04;
const T_FILE_END: u8 = 0x05;
const T_DONE: u8 = 0x06;
const T_ERROR: u8 = 0x07;

impl Frame {
    pub fn encode(&self) -> Vec<u8> {
        let mut p = Vec::new();
        match self {
            Frame::OpenOk {
                read_count,
                download_count,
                last_file_timestamp,
            } => {
                p.extend_from_slice(&read_count.to_le_bytes());
                p.extend_from_slice(&download_count.to_le_bytes());
                p.extend_from_slice(&(last_file_timestamp.len() as u16).to_le_bytes());
                p.extend_from_slice(last_file_timestamp.as_bytes());
            }
            Frame::Progress { done, total } => {
                p.extend_from_slice(&done.to_le_bytes());
                p.extend_from_slice(&total.to_le_bytes());
            }
            Frame::FileBegin { name, timestamp } => {
                p.extend_from_slice(&(name.len() as u16).to_le_bytes());
                p.extend_from_slice(name.as_bytes());
                p.extend_from_slice(&(timestamp.len() as u16).to_le_bytes());
                p.extend_from_slice(timestamp.as_bytes());
            }
            Frame::Record(data) => p.extend_from_slice(data),
            Frame::FileEnd { name } => {
                p.extend_from_slice(&(name.len() as u16).to_le_bytes());
                p.extend_from_slice(name.as_bytes());
            }
            Frame::Done => {}
            Frame::Error { code, message } => {
                p.extend_from_slice(&code.to_le_bytes());
                p.extend_from_slice(message.as_bytes());
            }
        }
        let mut out = Vec::with_capacity(5 + p.len());
        out.extend_from_slice(&(p.len() as u32 + 1).to_le_bytes());
        out.push(self.tag());
        out.extend_from_slice(&p);
        out
    }

    fn tag(&self) -> u8 {
        match self {
            Frame::OpenOk { .. } => T_OPEN_OK,
            Frame::Progress { .. } => T_PROGRESS,
            Frame::FileBegin { .. } => T_FILE_BEGIN,
            Frame::Record(_) => T_RECORD,
            Frame::FileEnd { .. } => T_FILE_END,
            Frame::Done => T_DONE,
            Frame::Error { .. } => T_ERROR,
        }
    }

    fn decode(tag: u8, p: &[u8]) -> io::Result<Self> {
        let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
        let get_i32 = |p: &[u8], off: usize| -> io::Result<i32> {
            let b: [u8; 4] = p
                .get(off..off + 4)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| bad("short i32"))?;
            Ok(i32::from_le_bytes(b))
        };
        let get_str = |p: &[u8], off: usize| -> io::Result<(String, usize)> {
            let n = p
                .get(off..off + 2)
                .and_then(|s| <[u8; 2]>::try_from(s).ok())
                .map(u16::from_le_bytes)
                .ok_or_else(|| bad("short len"))? as usize;
            let s = p
                .get(off + 2..off + 2 + n)
                .ok_or_else(|| bad("short str"))?;
            Ok((String::from_utf8_lossy(s).into_owned(), off + 2 + n))
        };
        Ok(match tag {
            T_OPEN_OK => Frame::OpenOk {
                read_count: get_i32(p, 0)?,
                download_count: get_i32(p, 4)?,
                last_file_timestamp: get_str(p, 8)?.0,
            },
            T_PROGRESS => Frame::Progress {
                done: get_i32(p, 0)?,
                total: get_i32(p, 4)?,
            },
            T_FILE_BEGIN => {
                let (name, off) = get_str(p, 0)?;
                let (timestamp, _) = get_str(p, off)?;
                Frame::FileBegin { name, timestamp }
            }
            T_RECORD => Frame::Record(p.to_vec()),
            T_FILE_END => Frame::FileEnd {
                name: get_str(p, 0)?.0,
            },
            T_DONE => Frame::Done,
            T_ERROR => Frame::Error {
                code: get_i32(p, 0)?,
                message: String::from_utf8_lossy(p.get(4..).unwrap_or(&[])).into_owned(),
            },
            _ => return Err(bad("unknown tag")),
        })
    }
}

/// 1 frame 読む。EOF (先頭すら無い) なら Ok(None)。
///
/// len は tag(1) + payload の長さ。bridge が壊れたときに巨大確保や
/// 範囲外アクセスで落ちないよう、上限を設けて検証する
/// (JVGets の最大バッファ 110KB より十分大きい値)。
const MAX_FRAME_LEN: usize = 1024 * 1024;

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Frame>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid frame length: {len}"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Frame::decode(buf[0], &buf[1..]).map(Some)
}

/// 1 frame 書く。
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, f: &Frame) -> io::Result<()> {
    w.write_all(&f.encode()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[tokio::test]
    async fn roundtrip() {
        let frames = vec![
            Frame::OpenOk {
                read_count: 12,
                download_count: 3,
                last_file_timestamp: "20200101120000".into(),
            },
            Frame::Progress { done: 1, total: 3 },
            Frame::FileBegin {
                name: "RASW200101120000.jvd".into(),
                timestamp: "20200101120000".into(),
            },
            Frame::Record(b"RA0123456789".to_vec()),
            Frame::Record(vec![0, 1, 2, 255]),
            Frame::FileEnd {
                name: "RASW200101120000.jvd".into(),
            },
            Frame::Done,
            Frame::Error {
                code: -301,
                message: "sample".into(),
            },
        ];
        let mut buf = Vec::new();
        for f in &frames {
            write_frame(&mut buf, f).await.ok();
        }
        let mut cur = Cursor::new(buf);
        for want in &frames {
            let got = read_frame(&mut cur).await.ok().flatten();
            assert_eq!(got.as_ref(), Some(want));
        }
        assert!(read_frame(&mut cur).await.ok().flatten().is_none());
    }

    #[tokio::test]
    async fn malformed_length_is_error_not_panic() {
        // len=0: tag すら無い壊れた frame
        let mut cur = Cursor::new(0u32.to_le_bytes().to_vec());
        assert!(read_frame(&mut cur).await.is_err());

        // 上限超過: 巨大確保を試みずにエラーになる
        let mut cur = Cursor::new((u32::MAX).to_le_bytes().to_vec());
        assert!(read_frame(&mut cur).await.is_err());
    }
}
