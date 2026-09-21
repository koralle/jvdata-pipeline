//! ホスト側の raw record ソース。
//!
//! - [`ProcessSource`]: bridge コマンド (既定: `docker compose exec wine wine ...`)
//!   を spawn し、stdout の frame をデコードする
//! - [`FixtureSource`]: ローカルディレクトリの .jvd 風ファイルを
//!   同じ frame 列として再生する (テスト/開発用)
//!
//! どちらも JV-Link 知識を持たず、`FetchRequest` を frame の stream に変換するだけ。

use crate::protocol::{Frame, read_frame};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Done 受信後に bridge プロセスの終了を待つ上限。
/// これを過ぎても終わらないプロセスは kill する (hang しない)。
const BRIDGE_EXIT_TIMEOUT: Duration = Duration::from_secs(30);

/// JVOpen 相当の 1 回の取得要求。
#[derive(Debug, Clone)]
pub struct FetchRequest {
    /// "RACE" 等のデータ種別 ID
    pub dataspec: String,
    /// JVOpen fromtime: `YYYYMMDDhhmmss[-YYYYMMDDhhmmss]`
    pub fromtime: String,
    /// JVOpen option (setup=4, normal=1)
    pub option: i32,
    /// セットアップ再開用: このファイル名まで JVSkip する (仕様書の再開手順)
    pub skip_to_file: Option<String>,
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("bridge spawn failed: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("bridge reported error {code}: {message}")]
    Bridge { code: i32, message: String },
    #[error("bridge process exited with {0}")]
    Exit(std::process::ExitStatus),
    #[error("stream ended without Done frame")]
    UnexpectedEof,
    #[error("bridge stderr: {0}")]
    Stderr(String),
    #[error("bridge command is empty")]
    EmptyCommand,
}

/// frame の stream。`next()` が None になるか Err が来たら終わり。
pub struct FetchStream {
    rx: mpsc::Receiver<Result<Frame, SourceError>>,
}

impl FetchStream {
    pub async fn next(&mut self) -> Option<Result<Frame, SourceError>> {
        self.rx.recv().await
    }
}

pub trait Source {
    /// 1 window 分の取得を開始する。frame は OpenOk → FileBegin/Record/FileEnd
    /// → Done の順に流れる。
    fn fetch(&self, req: &FetchRequest) -> FetchStream;
}

/// bridge コマンドを spawn するソース。
/// `cmd` は jvlink-bridge.exe までのコマンド列 (例: docker compose exec ... wine Z:\...)
pub struct ProcessSource {
    cmd: Vec<String>,
}

impl ProcessSource {
    /// 既定: リポジトリの `scripts/jvlink-bridge.sh` 経由で compose の
    /// wine サービスを呼ぶ (varlock で env を注入してから exec)。
    /// `JVDATA_BRIDGE_SCRIPT` でスクリプトパスを上書きできる。
    pub fn wine_container() -> Self {
        let script = std::env::var("JVDATA_BRIDGE_SCRIPT").unwrap_or_else(|_| {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../scripts/jvlink-bridge.sh"
            )
            .to_string()
        });
        Self { cmd: vec![script] }
    }

    /// `--source "<cmd>"` で指定する任意コマンド。空白区切り
    /// (Windows パス対応のため shell quoting はしない)。
    pub fn from_cmdline(cmdline: &str) -> Self {
        Self {
            cmd: cmdline.split_whitespace().map(String::from).collect(),
        }
    }
}

impl Source for ProcessSource {
    fn fetch(&self, req: &FetchRequest) -> FetchStream {
        let (tx, rx) = mpsc::channel(1024);
        // 空コマンド (--source "" や空の JVDATA_SOURCE) では spawn せず
        // 明示的に失敗させる (fetch 引数を連ねる前の base cmd が空かで判定)。
        let cmd_empty = self.cmd.is_empty();
        let mut cmd = self.cmd.clone();
        cmd.extend([
            "fetch".into(),
            "--dataspec".into(),
            req.dataspec.clone(),
            "--from".into(),
            req.fromtime.clone(),
            "--option".into(),
            req.option.to_string(),
        ]);
        if let Some(f) = &req.skip_to_file {
            cmd.extend(["--skip-to-file".into(), f.clone()]);
        }

        tokio::spawn(async move {
            // send が false を返す = receiver が drop 済み。
            // その場合はこれ以上読んでも誰も受け取らないので、
            // 即座にタスクを抜けて child を drop (kill_on_drop で kill) する。
            let send = |f: Frame| {
                let tx = tx.clone();
                async move { tx.send(Ok(f)).await.is_ok() }
            };
            let send_err = |e: SourceError| {
                let tx = tx.clone();
                async move { tx.send(Err(e)).await.is_ok() }
            };

            if cmd_empty {
                send_err(SourceError::EmptyCommand).await;
                return;
            }
            let Some((prog, args)) = cmd.split_first() else {
                send_err(SourceError::EmptyCommand).await;
                return;
            };
            let mut child = match Command::new(prog)
                .args(args)
                // stdin を閉じる。継承のままだと wine が入力待ちになり
                // stdout の frame が流れないことがある。
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                // FetchStream が途中で drop されても bridge プロセスが
                // 残り続けないよう、child の drop 時に kill する。
                .kill_on_drop(true)
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    send_err(SourceError::Spawn(e)).await;
                    return;
                }
            };

            let (Some(stdout), Some(mut stderr)) = (child.stdout.take(), child.stderr.take())
            else {
                return;
            };
            let mut stdout = BufReader::new(stdout);
            let stderr_task = tokio::spawn(async move {
                let mut buf = Vec::new();
                tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf)
                    .await
                    .map(|_| buf)
            });

            loop {
                match read_frame(&mut stdout).await {
                    Ok(Some(Frame::Error { code, message })) => {
                        if !send_err(SourceError::Bridge { code, message }).await {
                            return;
                        }
                        break;
                    }
                    Ok(Some(Frame::Done)) => {
                        if !send(Frame::Done).await {
                            return;
                        }
                        break;
                    }
                    Ok(Some(f)) => {
                        if !send(f).await {
                            return;
                        }
                    }
                    Ok(None) => {
                        if !send_err(SourceError::UnexpectedEof).await {
                            return;
                        }
                        break;
                    }
                    Err(e) => {
                        if !send_err(SourceError::Io(e)).await {
                            return;
                        }
                        break;
                    }
                }
            }

            match tokio::time::timeout(BRIDGE_EXIT_TIMEOUT, child.wait()).await {
                // Done 後も終了しない bridge は残さない
                Err(_) => {
                    warn!("bridge did not exit after Done; killing");
                    let _ = child.kill().await;
                }
                Ok(Ok(s)) if s.success() => {}
                Ok(Ok(s)) => {
                    let err = stderr_task.await.ok().and_then(|r| r.ok());
                    let detail = err
                        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
                        .filter(|s| !s.is_empty());
                    send_err(match detail {
                        Some(m) => SourceError::Stderr(m),
                        None => SourceError::Exit(s),
                    })
                    .await;
                }
                Ok(Err(e)) => {
                    send_err(SourceError::Io(e)).await;
                }
            }
        });
        FetchStream { rx }
    }
}

/// fixture ディレクトリを読むソース。
///
/// ディレクトリ内のファイルを manifest.txt (あれば) かファイル名順で並べ、
/// 各ファイルを CRLF 区切りの raw record 列として流す。
/// JV-Link 不使用で ingest → parse → normalize の縦筋を通すためのもの。
pub struct FixtureSource {
    dir: PathBuf,
}

impl FixtureSource {
    pub fn new(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }
}

impl Source for FixtureSource {
    fn fetch(&self, req: &FetchRequest) -> FetchStream {
        let (tx, rx) = mpsc::channel(1024);
        let dir = self.dir.clone();
        let req = req.clone();
        tokio::spawn(async move {
            let send = |r: Result<Frame, SourceError>| {
                let tx = tx.clone();
                async move { tx.send(r).await.ok() }
            };

            // manifest.txt があればその順、なければファイル名順
            let manifest = dir.join("manifest.txt");
            let files: Vec<PathBuf> = if manifest.exists() {
                match std::fs::read_to_string(&manifest) {
                    Ok(m) => m
                        .lines()
                        .filter(|l| !l.is_empty())
                        .map(|l| dir.join(l))
                        .collect(),
                    Err(e) => {
                        send(Err(SourceError::Io(e))).await;
                        return;
                    }
                }
            } else {
                let mut v: Vec<PathBuf> = match std::fs::read_dir(&dir) {
                    Ok(rd) => rd
                        .filter_map(|e| e.ok().map(|e| e.path()))
                        .filter(|p| p.is_file())
                        .collect(),
                    Err(e) => {
                        send(Err(SourceError::Io(e))).await;
                        return;
                    }
                };
                v.sort();
                v
            };

            // skip_to_file: そのファイルが先頭になるまで読み飛ばす
            // (JVSkip の再開手順と同じ意味: 指定ファイル自身は再読する)。
            // 対象が見つからない場合は bridge 実装と同じくエラーにする
            // (全部読み直すと再開位置を間違えたまま進んでしまう)。
            let files: Vec<PathBuf> = match &req.skip_to_file {
                Some(name) => match files.iter().position(|p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy() == name.as_str())
                        .unwrap_or(false)
                }) {
                    Some(i) => files[i..].to_vec(),
                    None => {
                        send(Err(SourceError::Bridge {
                            code: -9901,
                            message: format!("skip-to-file {name} not found in stream"),
                        }))
                        .await;
                        return;
                    }
                },
                None => files,
            };

            // JV-Link の lastfiletimestamp 相当: 最終ファイル名の 14 桁時刻。
            // (fixture ファイル名も本物と同じ "XXXXyyyymmddhhmmss.jvd" 形式)
            let last_file_timestamp = files
                .last()
                .and_then(|p| p.file_name())
                .map(|n| file_timestamp(&n.to_string_lossy()))
                .unwrap_or_default();

            send(Ok(Frame::OpenOk {
                read_count: files.len() as i32,
                download_count: 0,
                last_file_timestamp,
            }))
            .await;

            for path in &files {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let bytes = match std::fs::read(path) {
                    Ok(b) => b,
                    Err(e) => {
                        send(Err(SourceError::Io(e))).await;
                        return;
                    }
                };
                send(Ok(Frame::FileBegin {
                    timestamp: file_timestamp(&name),
                    name: name.clone(),
                }))
                .await;
                // CRLF 区切りでレコード分割。payload は CRLF を含めて保持。
                for rec in bytes.split_inclusive(|b| *b == b'\n') {
                    if !rec.is_empty() {
                        send(Ok(Frame::Record(rec.to_vec()))).await;
                    }
                }
                send(Ok(Frame::FileEnd { name })).await;
            }
            send(Ok(Frame::Done)).await;
            info!(dir = %dir.display(), "fixture stream done");
        });
        FetchStream { rx }
    }
}

/// JV-Link ファイル名 "XXXXyyyymmddhhmmss.jvd" から 14 桁の提供時刻を
/// 拾う。形が違う名前なら空文字 (= 時刻不明、ingest 側は空を無視する)。
fn file_timestamp(name: &str) -> String {
    let stem = name.strip_suffix(".jvd").unwrap_or(name);
    let tail: String = stem.chars().rev().take(14).collect();
    let tail: String = tail.chars().rev().collect();
    if tail.len() == 14 && tail.bytes().all(|b| b.is_ascii_digit()) {
        tail
    } else {
        String::new()
    }
}

/// 文字列から Source を作る CLI 用のヘルパ。
/// "fixture:DIR" → FixtureSource、それ以外は bridge コマンド文字列。
pub fn make_source(spec: &str) -> Box<dyn Source> {
    if let Some(dir) = spec.strip_prefix("fixture:") {
        Box::new(FixtureSource::new(dir))
    } else if spec == "wine" {
        Box::new(ProcessSource::wine_container())
    } else {
        warn!(cmd = spec, "custom bridge command");
        Box::new(ProcessSource::from_cmdline(spec))
    }
}
