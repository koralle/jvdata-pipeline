-- ingest の状態確認

-- 実行履歴
select id, mode, dataspec, window_start, window_end, status,
       records_read, records_stored, records_skipped, parse_errors,
       started_at, finished_at
from ingest.runs
order by id desc
limit 20;

-- dataspec / ウィンドウ別チェックポイント
select dataspec, window_start, window_end, mode, state,
       files_done, last_file, last_file_timestamp
from ingest.checkpoints
order by dataspec, window_start;

-- 通常取得 (option=1) のカーソル
select * from ingest.cursors;

-- raw 統計
select record_type, parse_state, count(*)
from ingest.raw_records
group by 1, 2
order by 1, 2;

select pg_size_pretty(pg_database_size(current_database())) as db_size;
