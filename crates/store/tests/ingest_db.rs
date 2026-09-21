//! PostgreSQL 統合テスト。`DATABASE_URL` が無い環境では skip する。
//!
//! 検証する不変条件:
//!   - raw_records の (filename, file_seq, payload_sha256) 冪等キー
//!   - checkpoint は commit と同一 tx でのみ進む (files_done)
//!   - 再開時、中断した前回 run が failed に畳まれる (running 残りしない)

use store::{CheckpointRow, NewRawRecord, Store};

fn rec(seq: i64, file_seq: i32, ty: &str, payload: &[u8]) -> NewRawRecord {
    NewRawRecord {
        seq,
        file_seq,
        record_type: ty.to_string(),
        data_kubun: Some("1".to_string()),
        make_date: None,
        payload: payload.to_vec(),
    }
}

/// DATABASE_URL 未設定なら None (skip)。設定済みなのに接続・migration に
/// 失敗した場合は Err で落とす: 失敗を skip に見せかけると、
/// migration の破損や接続設定ミスが CI で検出できない。
async fn db() -> Result<Option<Store>, sqlx::Error> {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        eprintln!("DATABASE_URL is not set; skipping DB integration test");
        return Ok(None);
    };
    let store = Store::connect(&url).await?;
    store
        .migrate()
        .await
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;
    Ok(Some(store))
}

async fn cleanup(store: &Store, ds: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update ingest.checkpoints set last_run_id = null
          where last_run_id in (select id from ingest.runs where dataspec = $1)",
    )
    .bind(ds)
    .execute(store.pool())
    .await?;
    sqlx::query("delete from ingest.checkpoints where dataspec = $1")
        .bind(ds)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from ingest.cursors where dataspec = $1")
        .bind(ds)
        .execute(store.pool())
        .await?;
    // parse_errors が raw_record_id を FK 参照するので先に消す
    // (normalize_pending は全 run の pending を処理するので、
    //   別テストの normalize がこの run のレコードに error 行を作り得る)
    sqlx::query(
        "delete from ingest.parse_errors
          where raw_record_id in (select id from ingest.raw_records
                                   where filename like $1 || '%')",
    )
    .bind(ds)
    .execute(store.pool())
    .await?;
    sqlx::query("delete from ingest.raw_records where filename like $1 || '%'")
        .bind(ds)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from ingest.runs where dataspec = $1")
        .bind(ds)
        .execute(store.pool())
        .await?;
    Ok(())
}

#[tokio::test]
async fn commit_is_idempotent_and_checkpoint_follows_commit() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTA";
    let ws = "20000101000000";
    cleanup(&store, ds).await?;

    store
        .seed_window(ds, ws, Some("20001231000000"), "setup")
        .await?;
    let run_id = store
        .begin_window(ds, ws, Some("20001231000000"), "setup", 4)
        .await?;

    let records = vec![
        rec(0, 0, "XX", b"XX1test-record-a"),
        rec(1, 1, "XX", b"XX1test-record-b"),
    ];
    let c1 = store
        .commit_file(run_id, ds, ws, "TESTAFILE01.jvd", &records)
        .await?;
    assert_eq!(c1.inserted, 2);
    assert_eq!(c1.skipped, 0);

    // 同一内容の再 commit は全件 skip
    let c2 = store
        .commit_file(run_id, ds, ws, "TESTAFILE01.jvd", &records)
        .await?;
    assert_eq!(c2.inserted, 0);
    assert_eq!(c2.skipped, 2);

    // files_done は「commit 済みの異なるファイル数」。同じファイルの
    // 再 commit (再開時の再読) は二重計上しない。
    let cp = store
        .checkpoints()
        .await?
        .into_iter()
        .find(|c| c.dataspec == ds && c.window_start == ws)
        .ok_or(sqlx::Error::RowNotFound)?;
    assert_eq!(cp.files_done, 1);
    assert_eq!(cp.last_file.as_deref(), Some("TESTAFILE01.jvd"));

    // 次のファイルを commit すると +1 される
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTAFILE02.jvd",
            &[rec(2, 0, "XX", b"XX1test-record-c")],
        )
        .await?;
    let cp = store
        .checkpoints()
        .await?
        .into_iter()
        .find(|c| c.dataspec == ds && c.window_start == ws)
        .ok_or(sqlx::Error::RowNotFound)?;
    assert_eq!(cp.files_done, 2);
    assert_eq!(cp.last_file.as_deref(), Some("TESTAFILE02.jvd"));

    store
        .finish_window(run_id, ds, ws, Some("20001231000000"), None)
        .await?;

    cleanup(&store, ds).await
}

#[tokio::test]
async fn resumed_window_supersedes_dead_running_run() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTB";
    let ws = "20010101000000";
    cleanup(&store, ds).await?;

    store
        .seed_window(ds, ws, Some("20011231000000"), "setup")
        .await?;
    let dead_run = store
        .begin_window(ds, ws, Some("20011231000000"), "setup", 4)
        .await?;
    // ここでプロセスが kill された想定: run=running, checkpoint=running のまま

    // 再開: 同じ window を begin し直す
    let new_run = store
        .begin_window(ds, ws, Some("20011231000000"), "setup", 4)
        .await?;

    let status: String = sqlx::query_scalar("select status from ingest.runs where id = $1")
        .bind(dead_run)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(status, "failed", "interrupted run must be folded to failed");

    store.finish_window(new_run, ds, ws, None, None).await?;

    cleanup(&store, ds).await
}

/// --to を延ばして backfill を再実行したとき、同じ window_start の
/// checkpoint が新しい window_end に更新され、done 済みなら pending に
/// 戻ること (延びた末尾分を取り直すため)。
#[tokio::test]
async fn seed_window_refreshes_changed_window_end() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTC";
    let ws = "20020101000000";
    cleanup(&store, ds).await?;

    async fn checkpoint(store: &Store, ds: &str, ws: &str) -> Result<CheckpointRow, sqlx::Error> {
        store
            .checkpoints()
            .await?
            .into_iter()
            .find(|c| c.dataspec == ds && c.window_start == ws)
            .ok_or(sqlx::Error::RowNotFound)
    }

    // pending window の end 変更 → end だけ更新、state は pending のまま
    store
        .seed_window(ds, ws, Some("20020630235959"), "setup")
        .await?;
    store
        .seed_window(ds, ws, Some("20021231235959"), "setup")
        .await?;
    let c = checkpoint(&store, ds, ws).await?;
    assert_eq!(c.window_end.as_deref(), Some("20021231235959"));
    assert_eq!(c.state, "pending");

    // done 済み window の範囲拡大 → pending に戻り、再開情報はリセット
    let run_id = store
        .begin_window(ds, ws, Some("20021231235959"), "setup", 4)
        .await?;
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTCFILE01.jvd",
            &[rec(0, 0, "XX", b"XX1x")],
        )
        .await?;
    store
        .finish_window(run_id, ds, ws, Some("20021231000000"), None)
        .await?;
    store.seed_window(ds, ws, None, "setup").await?; // open-ended に延ばした想定
    let c = checkpoint(&store, ds, ws).await?;
    assert_eq!(c.window_end, None);
    assert_eq!(c.state, "pending");
    assert_eq!(c.files_done, 0);
    assert_eq!(c.last_file, None);

    // 同じ end で再 seed → done は維持 (冪等)
    let run_id2 = store.begin_window(ds, ws, None, "setup", 4).await?;
    store
        .finish_window(
            run_id2,
            ds,
            ws,
            Some("20030101000000"),
            Some("20030101000000"),
        )
        .await?;
    store.seed_window(ds, ws, None, "setup").await?;
    let c = checkpoint(&store, ds, ws).await?;
    assert_eq!(c.state, "done");

    cleanup(&store, ds).await
}

/// ingest 用 advisory lock は同時に 1 セッションのみ。
/// guard を drop (= 接続を切る) すると解放される。
#[tokio::test]
async fn ingest_lock_is_exclusive_and_released_on_drop() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // ブロッキング版で直列化も兼ねる (他の DB テストはここで待つ)
    let lock = store.ingest_lock().await?;
    assert!(
        store.try_ingest_lock().await?.is_none(),
        "second session must not acquire the lock while the first holds it"
    );
    lock.unlock().await?;
    // unlock が効いていなければ blocking 取得がここで止まる。
    // (try_ だと他テストとのレースで稀に取れないことがある)
    let _lock = store.ingest_lock().await?;
    Ok(())
}

/// 差分 cursor (lastfiletimestamp) は単調増加。
/// 空文字・同値・過去の timestamp では後退しない。
#[tokio::test]
async fn cursor_never_regresses() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTF";
    cleanup(&store, ds).await?;

    // 空文字は「未取得」。cursor を作らない。
    store.advance_cursor(ds, "").await?;
    assert_eq!(store.cursor(ds).await?, None);

    store.advance_cursor(ds, "20200101120000").await?;
    assert_eq!(store.cursor(ds).await?.as_deref(), Some("20200101120000"));

    // 過去・同値・空はすべて無視される
    store.advance_cursor(ds, "20190101000000").await?;
    store.advance_cursor(ds, "20200101120000").await?;
    store.advance_cursor(ds, "").await?;
    assert_eq!(store.cursor(ds).await?.as_deref(), Some("20200101120000"));

    store.advance_cursor(ds, "20210101000000").await?;
    assert_eq!(store.cursor(ds).await?.as_deref(), Some("20210101000000"));

    // finish_window の cursor_ts も後退させない
    let ws = "20040101000000";
    store.seed_window(ds, ws, None, "setup").await?;
    let run_id = store.begin_window(ds, ws, None, "setup", 4).await?;
    store
        .finish_window(run_id, ds, ws, None, Some("20000101000000"))
        .await?;
    assert_eq!(store.cursor(ds).await?.as_deref(), Some("20210101000000"));

    cleanup(&store, ds).await
}

/// last_file は JVSkip の再開点。順序外の古いファイル名では後退しない。
#[tokio::test]
async fn last_file_never_regresses() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTL";
    let ws = "20050101000000";
    cleanup(&store, ds).await?;

    store.seed_window(ds, ws, None, "setup").await?;
    let run_id = store.begin_window(ds, ws, None, "setup", 4).await?;
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTLFILE09.jvd",
            &[rec(0, 0, "XX", b"XX1a")],
        )
        .await?;
    // 順序外の古いファイルを commit しても再開点は最後尾のまま
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTLFILE01.jvd",
            &[rec(1, 0, "XX", b"XX1b")],
        )
        .await?;
    let cp = store
        .checkpoints()
        .await?
        .into_iter()
        .find(|c| c.dataspec == ds && c.window_start == ws)
        .ok_or(sqlx::Error::RowNotFound)?;
    assert_eq!(cp.last_file.as_deref(), Some("TESTLFILE09.jvd"));

    cleanup(&store, ds).await
}

/// abandon_window: pending/failed/running を 'abandoned' にして
/// pending_windows の対象から外す。done/abandoned は変更しない。
/// 範囲を変えた再 seed で pending に復活する (再開情報はリセット)。
#[tokio::test]
async fn abandon_window_excludes_from_pending_and_reseeds_on_range_change()
-> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    let _lock = store.ingest_lock().await?;
    let ds = "TESTA";
    let ws = "20060101000000";
    cleanup(&store, ds).await?;

    async fn checkpoint(store: &Store, ds: &str, ws: &str) -> Result<CheckpointRow, sqlx::Error> {
        store
            .checkpoints()
            .await?
            .into_iter()
            .find(|c| c.dataspec == ds && c.window_start == ws)
            .ok_or(sqlx::Error::RowNotFound)
    }

    // 存在しない行は None
    assert_eq!(store.abandon_window(ds, ws).await?, None);

    // 失敗済み window を abandon → pending_windows から外れる
    store
        .seed_window(ds, ws, Some("20061231235959"), "setup")
        .await?;
    let run_id = store
        .begin_window(ds, ws, Some("20061231235959"), "setup", 4)
        .await?;
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTAFILE1.jvd",
            &[rec(0, 0, "XX", b"XX1a")],
        )
        .await?;
    store.fail_window(run_id, ds, ws, "boom").await?;
    assert_eq!(
        store.abandon_window(ds, ws).await?.as_deref(),
        Some("failed")
    );
    let c = checkpoint(&store, ds, ws).await?;
    assert_eq!(c.state, "abandoned");
    assert!(
        store
            .pending_windows()
            .await?
            .iter()
            .all(|c| !(c.dataspec == ds && c.window_start == ws))
    );

    // 同じ範囲で再 seed しても abandoned のまま
    store
        .seed_window(ds, ws, Some("20061231235959"), "setup")
        .await?;
    assert_eq!(checkpoint(&store, ds, ws).await?.state, "abandoned");

    // 範囲が変わる再 seed → pending に復活し再開情報はリセット
    store.seed_window(ds, ws, None, "setup").await?;
    let c = checkpoint(&store, ds, ws).await?;
    assert_eq!(c.state, "pending");
    assert_eq!(c.files_done, 0);
    assert_eq!(c.last_file, None);

    // done / abandoned は abandon_window が書き換えない
    let run_id2 = store.begin_window(ds, ws, None, "setup", 4).await?;
    store.finish_window(run_id2, ds, ws, None, None).await?;
    assert_eq!(store.abandon_window(ds, ws).await?.as_deref(), Some("done"));
    assert_eq!(checkpoint(&store, ds, ws).await?.state, "done");

    cleanup(&store, ds).await
}
