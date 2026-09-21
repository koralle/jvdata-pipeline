//! ingest.* への書き込み。トランザクション境界は「1 JV-Link ファイル」。
//! checkpoint は必ず同じトランザクション内で進める (commit より先に進めない)。

use crate::Store;
use sha2::{Digest, Sha256};
use time::Date;

/// 挿入前の raw record (client 側で連番・hash を付ける)。
#[derive(Debug)]
pub struct NewRawRecord {
    pub seq: i64,
    pub file_seq: i32,
    pub record_type: String,
    pub data_kubun: Option<String>,
    pub make_date: Option<Date>,
    pub payload: Vec<u8>,
}

impl NewRawRecord {
    pub fn sha256(&self) -> Vec<u8> {
        Sha256::digest(&self.payload).to_vec()
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CheckpointRow {
    pub dataspec: String,
    pub window_start: String,
    pub window_end: Option<String>,
    pub mode: String,
    pub state: String,
    pub files_done: i32,
    pub last_file: Option<String>,
    pub records_stored: i64,
    pub last_file_timestamp: Option<String>,
    pub error: Option<String>,
}

pub struct FileCommit {
    pub inserted: u64,
    pub skipped: u64,
}

impl Store {
    /// 計画した window を pending として登録。
    /// 既存行は基本的に維持するが、window_end が変わった場合は更新する。
    /// done/failed 済み window の範囲が変わった (= --to を延ばして再実行)
    /// ときは、末尾の未取得分を取り直すため pending に戻す (last_file は
    /// 残さない: 範囲が変わるとファイル列も変わり得るので最初から読み直す。
    /// 古い last_file で JVSkip 再開すると対象が stream に無くて詰む。
    /// 重複は raw_records の冪等キーで吸収される)。
    /// 'running' 中の window は進行中の run が持っているので触らない。
    pub async fn seed_window(
        &self,
        dataspec: &str,
        window_start: &str,
        window_end: Option<&str>,
        mode: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "insert into ingest.checkpoints as c
                 (dataspec, window_start, window_end, mode, state)
             values ($1, $2, $3, $4, 'pending')
             on conflict (dataspec, window_start) do update set
                 window_end = excluded.window_end,
                 state = case when c.state in ('done', 'failed') then 'pending'
                            else c.state end,
                 files_done = case when c.state in ('done', 'failed') then 0
                                 else c.files_done end,
                 last_file = case when c.state in ('done', 'failed') then null
                                else c.last_file end,
                 updated_at = now()
             where c.window_end is distinct from excluded.window_end",
        )
        .bind(dataspec)
        .bind(window_start)
        .bind(window_end)
        .bind(mode)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 実行待ちの window (pending / failed / running-放置) を start 順で返す。
    pub async fn pending_windows(&self) -> Result<Vec<CheckpointRow>, sqlx::Error> {
        sqlx::query_as::<_, CheckpointRow>(
            "select dataspec, window_start, window_end, mode, state, files_done,
                    last_file, records_stored, last_file_timestamp, error
               from ingest.checkpoints
              where state <> 'done'
              order by window_start",
        )
        .fetch_all(self.pool())
        .await
    }

    pub async fn checkpoints(&self) -> Result<Vec<CheckpointRow>, sqlx::Error> {
        sqlx::query_as::<_, CheckpointRow>(
            "select dataspec, window_start, window_end, mode, state, files_done,
                    last_file, records_stored, last_file_timestamp, error
               from ingest.checkpoints
              order by dataspec, window_start",
        )
        .fetch_all(self.pool())
        .await
    }

    /// 1 window の実行開始。run 行を作り checkpoint を running にする。
    /// 戻り値は run_id。
    pub async fn begin_window(
        &self,
        dataspec: &str,
        window_start: &str,
        window_end: Option<&str>,
        mode: &str,
        option: i32,
    ) -> Result<i64, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let run_id: i64 = sqlx::query_scalar(
            "insert into ingest.runs
                (dataspec, mode, window_start, window_end, jvopen_option, status)
             values ($1, $2, $3, $4, $5, 'running')
             returning id",
        )
        .bind(dataspec)
        .bind(mode)
        .bind(window_start)
        .bind(window_end)
        .bind(option)
        .fetch_one(&mut *tx)
        .await?;
        // 前回 run が running のまま残っている = プロセスが途中で死んだ。
        // この window の新規開始をもって死んだと断定できるので failed にする。
        sqlx::query(
            "update ingest.runs
                set status = 'failed', finished_at = now(),
                    error = 'interrupted: superseded by run ' || $1
              where status = 'running'
                and id = (select last_run_id from ingest.checkpoints
                           where dataspec = $2 and window_start = $3)",
        )
        .bind(run_id)
        .bind(dataspec)
        .bind(window_start)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "update ingest.checkpoints
                set state = 'running', last_run_id = $1, error = null, updated_at = now()
              where dataspec = $2 and window_start = $3",
        )
        .bind(run_id)
        .bind(dataspec)
        .bind(window_start)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(run_id)
    }

    /// JVOpen が返した値を run に記録する。
    pub async fn run_opened(
        &self,
        run_id: i64,
        read_count: i32,
        download_count: i32,
        last_file_timestamp: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "update ingest.runs
                set read_count = $1, download_count = $2, last_file_timestamp = $3
              where id = $4",
        )
        .bind(read_count)
        .bind(download_count)
        .bind(last_file_timestamp)
        .bind(run_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 1 ファイル分の raw_records を挿入し、run と checkpoint を同じ
    /// トランザクションで進める。commit 後にのみ checkpoint が進む。
    pub async fn commit_file(
        &self,
        run_id: i64,
        dataspec: &str,
        window_start: &str,
        filename: &str,
        records: &[NewRawRecord],
    ) -> Result<FileCommit, sqlx::Error> {
        let mut tx = self.pool().begin().await?;

        let n = records.len() as i64;
        let inserted = if records.is_empty() {
            0
        } else {
            let run_ids = vec![run_id; records.len()];
            let seqs: Vec<i64> = records.iter().map(|r| r.seq).collect();
            let file_seqs: Vec<i32> = records.iter().map(|r| r.file_seq).collect();
            let types: Vec<&str> = records.iter().map(|r| r.record_type.as_str()).collect();
            let kubuns: Vec<Option<&str>> =
                records.iter().map(|r| r.data_kubun.as_deref()).collect();
            let makes: Vec<Option<Date>> = records.iter().map(|r| r.make_date).collect();
            let payloads: Vec<&[u8]> = records.iter().map(|r| r.payload.as_slice()).collect();
            let shas: Vec<Vec<u8>> = records.iter().map(NewRawRecord::sha256).collect();
            sqlx::query(
                "insert into ingest.raw_records
                    (run_id, seq, filename, file_seq, record_type, data_kubun,
                     make_date, payload, payload_sha256)
                 select * from unnest(
                    $1::bigint[], $2::bigint[], $3::text[], $4::int[],
                    $5::text[], $6::text[], $7::date[], $8::bytea[], $9::bytea[])
                 on conflict (filename, file_seq, payload_sha256) do nothing",
            )
            .bind(&run_ids)
            .bind(&seqs)
            .bind(vec![filename; records.len()])
            .bind(&file_seqs)
            .bind(&types)
            .bind(&kubuns)
            .bind(&makes)
            .bind(&payloads)
            .bind(&shas)
            .execute(&mut *tx)
            .await?
            .rows_affected()
        };

        sqlx::query(
            "update ingest.runs
                set files_done = files_done + 1,
                    records_read = records_read + $1,
                    records_stored = records_stored + $2,
                    records_skipped = records_skipped + ($1 - $2)
              where id = $3",
        )
        .bind(n)
        .bind(inserted as i64)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;

        // files_done は「commit 済みの異なるファイル数」。再開時に last_file
        // 自身を再読して commit し直すことがあるが、その場合は二重計上しない。
        // last_file は JVSkip の再開点なので後退させない (ファイル名は
        // 提供時刻順にソート可能)。
        sqlx::query(
            "update ingest.checkpoints
                set files_done = files_done +
                        case when last_file is distinct from $1 then 1 else 0 end,
                    last_file = case
                        when last_file is null or $1 > last_file then $1
                        else last_file end,
                    records_stored = records_stored + $2,
                    updated_at = now()
              where dataspec = $3 and window_start = $4",
        )
        .bind(filename)
        .bind(inserted as i64)
        .bind(dataspec)
        .bind(window_start)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(FileCommit {
            inserted,
            skipped: n as u64 - inserted,
        })
    }

    /// window 正常終了。checkpoint=done、run=done、cursor 更新を 1 tx で。
    /// `cursor_ts` は差分取得 (option=1) の次回 fromtime になる値。
    /// セットアップ最終 window では JVOpen の lastfiletimestamp、データが
    /// 0 件 (-1) で終わった場合は呼び出し側で window_start に丸めて渡す。
    pub async fn finish_window(
        &self,
        run_id: i64,
        dataspec: &str,
        window_start: &str,
        last_file_timestamp: Option<&str>,
        cursor_ts: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        // last_file_timestamp は提供時刻。空・過去の値で既存値を後退させない
        // (timestamp は YYYYMMDDhhmmss で辞書順 = 時刻順)。
        sqlx::query(
            "update ingest.checkpoints
                set state = 'done', updated_at = now(),
                    last_file_timestamp = case
                        when $1 is null or $1 = '' then last_file_timestamp
                        when last_file_timestamp is null or $1 > last_file_timestamp
                            then $1
                        else last_file_timestamp end
              where dataspec = $2 and window_start = $3",
        )
        .bind(last_file_timestamp)
        .bind(dataspec)
        .bind(window_start)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "update ingest.runs
                set status = 'done', finished_at = now(),
                    last_file_timestamp = case
                        when $1 is null or $1 = '' then last_file_timestamp
                        when last_file_timestamp is null or $1 > last_file_timestamp
                            then $1
                        else last_file_timestamp end
              where id = $2",
        )
        .bind(last_file_timestamp)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        // セットアップ最終 window の lastfiletimestamp が差分取得の起点。
        // cursor は次回 fromtime なので、空・既存以下の値で後退させない。
        if let Some(ts) = cursor_ts.filter(|t| !t.is_empty()) {
            sqlx::query(
                "insert into ingest.cursors (dataspec, last_file_timestamp, updated_at)
                 values ($1, $2, now())
                 on conflict (dataspec) do update
                   set last_file_timestamp = excluded.last_file_timestamp,
                       updated_at = excluded.updated_at
                   where excluded.last_file_timestamp
                       > ingest.cursors.last_file_timestamp",
            )
            .bind(dataspec)
            .bind(ts)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// window 失敗。checkpoint=failed (files_done は巻き戻さない)、run=failed。
    pub async fn fail_window(
        &self,
        run_id: i64,
        dataspec: &str,
        window_start: &str,
        error: &str,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        sqlx::query(
            "update ingest.checkpoints
                set state = 'failed', error = $1, updated_at = now()
              where dataspec = $2 and window_start = $3",
        )
        .bind(error)
        .bind(dataspec)
        .bind(window_start)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "update ingest.runs set status = 'failed', error = $1, finished_at = now()
              where id = $2",
        )
        .bind(error)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// 全 dataspec の差分 cursor (lastfiletimestamp)。resume が使う。
    pub async fn all_cursors(&self) -> Result<Vec<(String, String)>, sqlx::Error> {
        sqlx::query_as::<_, (String, String)>(
            "select dataspec, last_file_timestamp from ingest.cursors order by dataspec",
        )
        .fetch_all(self.pool())
        .await
    }

    /// 差分 cursor (dataspec → lastfiletimestamp)。
    pub async fn cursor(&self, dataspec: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>(
            "select last_file_timestamp from ingest.cursors where dataspec = $1",
        )
        .bind(dataspec)
        .fetch_optional(self.pool())
        .await
    }

    /// 差分取得成功後に cursor を進める。cursor は次回 JVOpen の
    /// fromtime なので後退させてはいけない (後退 = 同じデータを全件
    /// 取り直し。空文字は「未取得」を表すので保存しない)。
    pub async fn advance_cursor(&self, dataspec: &str, ts: &str) -> Result<(), sqlx::Error> {
        if ts.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "insert into ingest.cursors (dataspec, last_file_timestamp, updated_at)
             values ($1, $2, now())
             on conflict (dataspec) do update
               set last_file_timestamp = excluded.last_file_timestamp,
                   updated_at = excluded.updated_at
               where excluded.last_file_timestamp
                   > ingest.cursors.last_file_timestamp",
        )
        .bind(dataspec)
        .bind(ts)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    /// 差分取得用の run (window ではない) を作る。
    pub async fn begin_diff_run(&self, dataspec: &str, from_ts: &str) -> Result<i64, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let run_id: i64 = sqlx::query_scalar(
            "insert into ingest.runs
                (dataspec, mode, window_start, jvopen_option, status)
             values ($1, 'normal', $2, 1, 'running')
             returning id",
        )
        .bind(dataspec)
        .bind(from_ts)
        .fetch_one(&mut *tx)
        .await?;
        // 同 dataspec の差分 run は直列。前のが running 残りなら中断死なので畳む。
        sqlx::query(
            "update ingest.runs
                set status = 'failed', finished_at = now(),
                    error = 'interrupted: superseded by run ' || $1
              where dataspec = $2 and mode = 'normal' and status = 'running' and id <> $1",
        )
        .bind(run_id)
        .bind(dataspec)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(run_id)
    }

    /// diff run 内で 1 ファイル commit (checkpoint は触らない)。
    pub async fn commit_diff_file(
        &self,
        run_id: i64,
        filename: &str,
        records: &[NewRawRecord],
    ) -> Result<FileCommit, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let n = records.len() as i64;
        let inserted = if records.is_empty() {
            0
        } else {
            let run_ids = vec![run_id; records.len()];
            let seqs: Vec<i64> = records.iter().map(|r| r.seq).collect();
            let file_seqs: Vec<i32> = records.iter().map(|r| r.file_seq).collect();
            let types: Vec<&str> = records.iter().map(|r| r.record_type.as_str()).collect();
            let kubuns: Vec<Option<&str>> =
                records.iter().map(|r| r.data_kubun.as_deref()).collect();
            let makes: Vec<Option<Date>> = records.iter().map(|r| r.make_date).collect();
            let payloads: Vec<&[u8]> = records.iter().map(|r| r.payload.as_slice()).collect();
            let shas: Vec<Vec<u8>> = records.iter().map(NewRawRecord::sha256).collect();
            sqlx::query(
                "insert into ingest.raw_records
                    (run_id, seq, filename, file_seq, record_type, data_kubun,
                     make_date, payload, payload_sha256)
                 select * from unnest(
                    $1::bigint[], $2::bigint[], $3::text[], $4::int[],
                    $5::text[], $6::text[], $7::date[], $8::bytea[], $9::bytea[])
                 on conflict (filename, file_seq, payload_sha256) do nothing",
            )
            .bind(&run_ids)
            .bind(&seqs)
            .bind(vec![filename; records.len()])
            .bind(&file_seqs)
            .bind(&types)
            .bind(&kubuns)
            .bind(&makes)
            .bind(&payloads)
            .bind(&shas)
            .execute(&mut *tx)
            .await?
            .rows_affected()
        };
        sqlx::query(
            "update ingest.runs
                set files_done = files_done + 1,
                    records_read = records_read + $1,
                    records_stored = records_stored + $2,
                    records_skipped = records_skipped + ($1 - $2)
              where id = $3",
        )
        .bind(n)
        .bind(inserted as i64)
        .bind(run_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(FileCommit {
            inserted,
            skipped: n as u64 - inserted,
        })
    }

    pub async fn finish_diff_run(
        &self,
        run_id: i64,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "update ingest.runs set status = $1, error = $2, finished_at = now()
              where id = $3",
        )
        .bind(status)
        .bind(error)
        .bind(run_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}
