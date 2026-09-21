-- checkpoints.state に 'abandoned' を追加する。
-- 'abandoned' は「実行を諦めた window」: pending_windows の対象外で、
-- resume/backfill が毎回リトライして失敗し続ける window を
-- `jvdata abandon` で隔離するための状態。
-- done とは違い取得完了を意味しない (cursor も進まない)。
-- 範囲を変えて backfill を再実行 (window_end 変更による re-seed)
-- すると pending に戻るので、復活させたい場合は --to を変えて seed する。
alter table ingest.checkpoints
    drop constraint checkpoints_state_check,
    add constraint checkpoints_state_check
        check (state in ('pending', 'running', 'done', 'failed', 'abandoned'));
