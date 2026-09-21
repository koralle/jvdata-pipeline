//! `jvdata status` 用の集計クエリ。

use crate::Store;
use sqlx::Row;

#[derive(Debug)]
pub struct StatusReport {
    pub runs_running: i64,
    pub runs_done: i64,
    pub runs_failed: i64,
    pub windows_pending: i64,
    pub windows_done: i64,
    pub windows_failed: i64,
    pub windows_abandoned: i64,
    pub raw_records: i64,
    pub raw_pending_parse: i64,
    pub parse_errors: i64,
    pub jv_races: i64,
    pub jv_entries: i64,
    pub jv_horses: i64,
    pub jv_payouts: i64,
    pub last_checkpoints: Vec<(String, String, String, i32, Option<String>)>,
    pub cursors: Vec<(String, String)>,
    /// 現在 running の run (id, dataspec, window_start, mode)
    pub current_runs: Vec<(i64, String, String, String)>,
}

impl Store {
    pub async fn status(&self) -> Result<StatusReport, sqlx::Error> {
        let run_row = sqlx::query(
            "select
                count(*) filter (where status = 'running')::bigint,
                count(*) filter (where status = 'done')::bigint,
                count(*) filter (where status = 'failed')::bigint
               from ingest.runs",
        )
        .fetch_one(self.pool())
        .await?;

        let win_row = sqlx::query(
            "select
                count(*) filter (where state = 'pending' or state = 'running')::bigint,
                count(*) filter (where state = 'done')::bigint,
                count(*) filter (where state = 'failed')::bigint,
                count(*) filter (where state = 'abandoned')::bigint
               from ingest.checkpoints",
        )
        .fetch_one(self.pool())
        .await?;

        let raw_row = sqlx::query(
            "select count(*)::bigint,
                    count(*) filter (where parse_state = 0)::bigint
               from ingest.raw_records",
        )
        .fetch_one(self.pool())
        .await?;

        let parse_errors: i64 =
            sqlx::query_scalar("select count(*)::bigint from ingest.parse_errors")
                .fetch_one(self.pool())
                .await?;
        let jv_races: i64 = sqlx::query_scalar("select count(*)::bigint from jv.races")
            .fetch_one(self.pool())
            .await?;
        let jv_entries: i64 = sqlx::query_scalar("select count(*)::bigint from jv.entries")
            .fetch_one(self.pool())
            .await?;
        let jv_horses: i64 = sqlx::query_scalar("select count(*)::bigint from jv.horses")
            .fetch_one(self.pool())
            .await?;
        let jv_payouts: i64 = sqlx::query_scalar("select count(*)::bigint from jv.payouts")
            .fetch_one(self.pool())
            .await?;

        let last_checkpoints = sqlx::query(
            "select dataspec, window_start, state, files_done, last_file
               from ingest.checkpoints
              order by updated_at desc
              limit 10",
        )
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>(0),
                r.get::<String, _>(1),
                r.get::<String, _>(2),
                r.get::<i32, _>(3),
                r.get::<Option<String>, _>(4),
            )
        })
        .collect();

        let cursors = sqlx::query(
            "select dataspec, last_file_timestamp from ingest.cursors order by dataspec",
        )
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(|r| (r.get::<String, _>(0), r.get::<String, _>(1)))
        .collect();

        let current_runs = sqlx::query(
            "select id, dataspec, window_start, mode
               from ingest.runs
              where status = 'running'
              order by id",
        )
        .fetch_all(self.pool())
        .await?
        .into_iter()
        .map(|r| {
            (
                r.get::<i64, _>(0),
                r.get::<String, _>(1),
                r.get::<String, _>(2),
                r.get::<String, _>(3),
            )
        })
        .collect();

        Ok(StatusReport {
            runs_running: run_row.get(0),
            runs_done: run_row.get(1),
            runs_failed: run_row.get(2),
            windows_pending: win_row.get(0),
            windows_done: win_row.get(1),
            windows_failed: win_row.get(2),
            windows_abandoned: win_row.get(3),
            raw_records: raw_row.get(0),
            raw_pending_parse: raw_row.get(1),
            parse_errors,
            jv_races,
            jv_entries,
            jv_horses,
            jv_payouts,
            last_checkpoints,
            cursors,
            current_runs,
        })
    }
}
