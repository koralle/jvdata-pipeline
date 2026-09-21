//! window → bridge → raw_records → checkpoint の実行ループ。
//!
//! 不変条件: checkpoint は DB commit と同じトランザクションでのみ進む
//! (store::commit_file 内)。この層は副作用の orchestration のみを持つ。
//!
//! 再開は JV-Link 仕様の手順どおり「同じパラメータで JVOpen → JVSkip で
//! 最後に保存したファイル名まで読み飛ばす」。JVSkip は bridge 側が行い、
//! この層は単に来た frame を保存するだけ (重複は raw_records の
//! unique 制約で冪等に吸収される)。

use bridge::protocol::Frame;
use bridge::source::{FetchRequest, Source, SourceError};
use jvdata_core::dataspec::Dataspec;
use jvdata_core::plan::Window;
use jvdata_core::record::RecordHead;
use std::time::Instant;
use store::{NewRawRecord, Store};
use time::Date;
use tracing::{info, warn};

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("source: {0}")]
    Source(#[from] SourceError),
}

/// raw_records の (record_type, data_kubun, make_date) を取り出す。
/// 共通ヘッダ全体の parse に失敗しても spec コード (先頭 2 byte) だけは
/// 拾う: "RA" 等が取れれば normalize 側で正式な parse error として
/// parse_errors に記録される。先頭 2 byte すら無いなら空文字
/// (normalize では未対応種別として skip される)。
fn raw_head(payload: &[u8]) -> (String, Option<String>, Option<Date>) {
    match RecordHead::parse(payload) {
        Ok(h) => (h.record_type, Some(h.data_kubun), h.make_date),
        Err(_) => (
            jvdata_core::parse::record_type(payload)
                .map(str::to_string)
                .unwrap_or_default(),
            None,
            None,
        ),
    }
}

/// window 1 本を取得して ingest.* に保存する。
/// `resume_file` = 前回までに commit 済みの最終ファイル名 (あれば)。
pub async fn run_window(
    store: &Store,
    source: &dyn Source,
    window: &Window,
    resume_file: Option<&str>,
) -> Result<(), IngestError> {
    let (dataspec, window_start) = window.key();
    let run_id = store
        .begin_window(
            &dataspec,
            &window_start,
            window.end.as_deref(),
            "setup",
            Dataspec::SETUP_OPTION,
        )
        .await?;
    let started = Instant::now();
    info!(dataspec, fromtime = %window.fromtime(), run_id, "window start");

    let result = run_window_inner(store, source, window, run_id, resume_file).await;
    match &result {
        Ok(()) => info!(dataspec, run_id, elapsed = ?started.elapsed(), "window done"),
        Err(e) => {
            warn!(dataspec, run_id, error = %e, "window failed");
            // fail 記録の失敗で元のエラーを隠さない
            if let Err(db_err) = store
                .fail_window(run_id, &dataspec, &window_start, &e.to_string())
                .await
            {
                warn!(error = %db_err, "failed to record window failure");
            }
        }
    }
    result
}

async fn run_window_inner(
    store: &Store,
    source: &dyn Source,
    window: &Window,
    run_id: i64,
    resume_file: Option<&str>,
) -> Result<(), IngestError> {
    let (dataspec, window_start) = window.key();
    let req = FetchRequest {
        dataspec: window.dataspec.id().to_string(),
        fromtime: window.fromtime(),
        option: Dataspec::SETUP_OPTION,
        skip_to_file: resume_file.map(String::from),
    };

    let mut stream = source.fetch(&req);
    let mut cur_file = String::new();
    let mut file_records: Vec<NewRawRecord> = Vec::new();
    let mut seq: i64 = 0;
    let mut last_ts: Option<String> = None;
    let mut saw_done = false;

    while let Some(frame) = stream.next().await {
        match frame.map_err(IngestError::Source)? {
            Frame::OpenOk {
                read_count,
                download_count,
                last_file_timestamp,
            } => {
                info!(read_count, download_count, "JVOpen ok");
                store
                    .run_opened(run_id, read_count, download_count, &last_file_timestamp)
                    .await?;
                if !last_file_timestamp.is_empty() {
                    last_ts = Some(last_file_timestamp);
                }
            }
            Frame::Progress { done, total } => {
                info!(done, total, "downloading");
            }
            Frame::FileBegin { name, .. } => {
                // 前ファイルの FileEnd が欠けた場合の防御: 残っている
                // records を前ファイル名で commit してから新ファイルへ。
                // 無いと前ファイルのレコードが新ファイル名で保存される。
                if !file_records.is_empty() {
                    warn!(file = %cur_file, "FileEnd missing; committing leftover records");
                    store
                        .commit_file(run_id, &dataspec, &window_start, &cur_file, &file_records)
                        .await?;
                    file_records.clear();
                }
                cur_file = name;
            }
            Frame::Record(payload) => {
                let (record_type, data_kubun, make_date) = raw_head(&payload);
                file_records.push(NewRawRecord {
                    seq,
                    file_seq: file_records.len() as i32,
                    record_type,
                    data_kubun,
                    make_date,
                    payload,
                });
                seq += 1;
            }
            Frame::FileEnd { name } => {
                let commit = store
                    .commit_file(run_id, &dataspec, &window_start, &name, &file_records)
                    .await?;
                info!(
                    file = %name, records = file_records.len(),
                    inserted = commit.inserted, skipped = commit.skipped,
                    "file committed"
                );
                file_records.clear();
                cur_file.clear();
            }
            Frame::Done => {
                saw_done = true;
                break;
            }
            Frame::Error { code, message } => {
                return Err(SourceError::Bridge { code, message }.into());
            }
        }
    }

    // Done なしで stream が閉じた = 途中で死んだ。window を done にすると
    // 未読ファイルが欠損するので必ず失敗にする。
    if !saw_done {
        return Err(IngestError::Source(SourceError::UnexpectedEof));
    }

    // FileEnd なしで Done に来た場合の残りを commit
    if !file_records.is_empty() {
        store
            .commit_file(run_id, &dataspec, &window_start, &cur_file, &file_records)
            .await?;
    }

    // セットアップ最終 window (end=None) の lastfiletimestamp が差分取得の起点。
    // JVOpen が -1 (該当データなし) で lastts を返さなかった window は
    // window_start に丸める: cursor を残さないとその dataspec は resume で
    // 差分取得が一度も走らない。fromtime は「その時刻より大きいファイル」
    // なので window_start で window 全体をカバーできる (重複は冪等キーで吸収)。
    let cursor_ts = window
        .end
        .is_none()
        .then(|| last_ts.as_deref().unwrap_or(window_start.as_str()));
    store
        .finish_window(
            run_id,
            &dataspec,
            &window_start,
            last_ts.as_deref(),
            cursor_ts,
        )
        .await?;
    Ok(())
}

/// 差分取得 (option=1)。cursor の lastfiletimestamp から新しいファイルを取る。
pub async fn run_diff(
    store: &Store,
    source: &dyn Source,
    dataspec: Dataspec,
    from_ts: &str,
) -> Result<(), IngestError> {
    let run_id = store.begin_diff_run(dataspec.id(), from_ts).await?;
    let started = Instant::now();
    info!(dataspec = %dataspec, from_ts, run_id, "diff fetch start");

    let result = run_diff_inner(store, source, dataspec, run_id, from_ts).await;
    match &result {
        Ok(()) => info!(dataspec = %dataspec, run_id, elapsed = ?started.elapsed(), "diff done"),
        Err(e) => {
            warn!(dataspec = %dataspec, run_id, error = %e, "diff failed");
            if let Err(db_err) = store
                .finish_diff_run(run_id, "failed", Some(&e.to_string()))
                .await
            {
                warn!(error = %db_err, "failed to record diff failure");
            }
        }
    }
    result
}

async fn run_diff_inner(
    store: &Store,
    source: &dyn Source,
    dataspec: Dataspec,
    run_id: i64,
    from_ts: &str,
) -> Result<(), IngestError> {
    let req = FetchRequest {
        dataspec: dataspec.id().to_string(),
        fromtime: from_ts.to_string(),
        option: Dataspec::NORMAL_OPTION,
        skip_to_file: None,
    };
    let mut stream = source.fetch(&req);
    let mut file_records: Vec<NewRawRecord> = Vec::new();
    let mut seq: i64 = 0;
    let mut cur_file = String::new();
    // 差分の再開位置。JVOpen が返す lastfiletimestamp は
    // 「今回取得した最終ファイルの timestamp」= 次回 fromtime に渡す値。
    // (FileBegin の m_CurrentFileTimestamp は読み中ファイルの時刻で、
    // 再開トークンとは別物)
    let mut new_cursor = String::new();
    let mut saw_done = false;

    while let Some(frame) = stream.next().await {
        match frame.map_err(IngestError::Source)? {
            Frame::OpenOk {
                read_count,
                download_count,
                last_file_timestamp,
            } => {
                store
                    .run_opened(run_id, read_count, download_count, &last_file_timestamp)
                    .await?;
                if !last_file_timestamp.is_empty() {
                    new_cursor = last_file_timestamp;
                }
            }
            Frame::FileBegin { name, .. } => {
                if !file_records.is_empty() {
                    warn!(file = %cur_file, "FileEnd missing; committing leftover records");
                    store
                        .commit_diff_file(run_id, &cur_file, &file_records)
                        .await?;
                    file_records.clear();
                }
                cur_file = name;
            }
            Frame::Record(payload) => {
                let (record_type, data_kubun, make_date) = raw_head(&payload);
                file_records.push(NewRawRecord {
                    seq,
                    file_seq: file_records.len() as i32,
                    record_type,
                    data_kubun,
                    make_date,
                    payload,
                });
                seq += 1;
            }
            Frame::FileEnd { name } => {
                let c = store.commit_diff_file(run_id, &name, &file_records).await?;
                info!(file = %name, records = file_records.len(), inserted = c.inserted, "diff file committed");
                file_records.clear();
                cur_file.clear();
            }
            Frame::Done => {
                saw_done = true;
                break;
            }
            Frame::Progress { .. } => {}
            Frame::Error { code, message } => {
                return Err(SourceError::Bridge { code, message }.into());
            }
        }
    }

    // Done なしの終了は失敗。ここで cursor を進めると未取得分が欠損する。
    if !saw_done {
        return Err(IngestError::Source(SourceError::UnexpectedEof));
    }

    if !file_records.is_empty() {
        store
            .commit_diff_file(run_id, &cur_file, &file_records)
            .await?;
    }

    if !new_cursor.is_empty() {
        store.advance_cursor(dataspec.id(), &new_cursor).await?;
    }
    store.finish_diff_run(run_id, "done", None).await?;
    Ok(())
}
