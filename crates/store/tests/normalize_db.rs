//! normalize (raw_records → jv.*) の PostgreSQL 統合テスト。
//! `DATABASE_URL` が無い環境では skip する。
//!
//! 検証する不変条件:
//!   - データ区分=0 (該当レコード削除) は jv.* 側の行を削除する
//!   - parse 失敗は parse_errors に記録され、同 batch の他レコードを止めない
//!   - parse_errors は raw_record 単位で 1 行 (reparse で重複しない)

use store::{NewRawRecord, Store};

fn rec(seq: i64, file_seq: i32, ty: &str, kubun: &str, payload: &[u8]) -> NewRawRecord {
    NewRawRecord {
        seq,
        file_seq,
        record_type: ty.to_string(),
        data_kubun: Some(kubun.to_string()),
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
    sqlx::query("delete from ingest.parse_errors where run_id in (select id from ingest.runs where dataspec = $1)")
        .bind(ds)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from ingest.raw_records where run_id in (select id from ingest.runs where dataspec = $1)")
        .bind(ds)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from ingest.runs where dataspec = $1")
        .bind(ds)
        .execute(store.pool())
        .await?;
    Ok(())
}

/// データ区分=0 のレコードは jv.* 側の対応行を削除する
/// (JV-Data 仕様書: 0 = 該当レコード削除)。
#[tokio::test]
async fn data_kubun_zero_deletes_jv_row() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTD";
    let ws = "20030101000000";
    let race_key = "2030010599010111"; // ymd=20300105 jyo=99 kaiji=01 nichiji=01 race=11
    let ketto = "9999999999";
    cleanup(&store, ds).await?;
    sqlx::query("delete from jv.races where race_key = $1")
        .bind(race_key)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from jv.entries where race_key = $1")
        .bind(race_key)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from jv.horses where ketto_num = $1")
        .bind(ketto)
        .execute(store.pool())
        .await?;

    store.seed_window(ds, ws, None, "setup").await?;
    let run_id = store.begin_window(ds, ws, None, "setup", 4).await?;

    // RA + SE + UM を kubun=7 (成績) / 1 (新規) で投入して normalize
    let ra = bridge::fixture::ra("20300105", "99", "01", "01", "11", "テスト");
    let se = bridge::fixture::se(
        "20300105",
        "99",
        "01",
        "01",
        "11",
        3,
        ketto,
        "テストサン",
        "00866",
        1,
        1335,
        121,
        4,
    );
    let um = bridge::fixture::um(ketto, "テストサン", "ドゥラメンテ", "母", "20200315");
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTDFILE01.jvd",
            &[
                rec(0, 0, "RA", "7", &ra),
                rec(1, 1, "SE", "7", &se),
                rec(2, 2, "UM", "1", &um),
            ],
        )
        .await?;
    store.normalize_pending().await?;

    let races: i64 = sqlx::query_scalar("select count(*) from jv.races where race_key = $1")
        .bind(race_key)
        .fetch_one(store.pool())
        .await?;
    let entries: i64 =
        sqlx::query_scalar("select count(*) from jv.entries where race_key = $1 and umaban = '03'")
            .bind(race_key)
            .fetch_one(store.pool())
            .await?;
    let horses: i64 = sqlx::query_scalar("select count(*) from jv.horses where ketto_num = $1")
        .bind(ketto)
        .fetch_one(store.pool())
        .await?;
    assert_eq!((races, entries, horses), (1, 1, 1));

    // 同じキーの kubun=0 レコードを投入 → jv.* の行が消える
    let mut ra_del = ra.clone();
    ra_del[2] = b'0';
    let mut se_del = se.clone();
    se_del[2] = b'0';
    let mut um_del = um.clone();
    um_del[2] = b'0';
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTDFILE02.jvd",
            &[
                rec(3, 0, "RA", "0", &ra_del),
                rec(4, 1, "SE", "0", &se_del),
                rec(5, 2, "UM", "0", &um_del),
            ],
        )
        .await?;
    store.normalize_pending().await?;

    let races: i64 = sqlx::query_scalar("select count(*) from jv.races where race_key = $1")
        .bind(race_key)
        .fetch_one(store.pool())
        .await?;
    let entries: i64 =
        sqlx::query_scalar("select count(*) from jv.entries where race_key = $1 and umaban = '03'")
            .bind(race_key)
            .fetch_one(store.pool())
            .await?;
    let horses: i64 = sqlx::query_scalar("select count(*) from jv.horses where ketto_num = $1")
        .bind(ketto)
        .fetch_one(store.pool())
        .await?;
    assert_eq!((races, entries, horses), (0, 0, 0));

    cleanup(&store, ds).await
}

/// parse 失敗レコードは parse_errors に記録され、同 batch の
/// 他レコードの反映を止めない。reparse しても parse_errors は増殖しない。
#[tokio::test]
async fn parse_failure_is_isolated_per_record() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTE";
    let ws = "20030101000000";
    let race_key = "2030010599010112";
    cleanup(&store, ds).await?;
    sqlx::query("delete from jv.races where race_key = $1")
        .bind(race_key)
        .execute(store.pool())
        .await?;

    store.seed_window(ds, ws, None, "setup").await?;
    let run_id = store.begin_window(ds, ws, None, "setup", 4).await?;

    // 先頭が壊れた RA (短すぎて parse 不能) + 正常 RA を 1 batch で。
    let bad = b"RA7";
    let good = bridge::fixture::ra("20300105", "99", "01", "01", "12", "テスト");
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTEFILE01.jvd",
            &[rec(0, 0, "RA", "7", bad), rec(1, 1, "RA", "7", &good)],
        )
        .await?;
    store.normalize_pending().await?;

    let races: i64 = sqlx::query_scalar("select count(*) from jv.races where race_key = $1")
        .bind(race_key)
        .fetch_one(store.pool())
        .await?;
    assert_eq!(
        races, 1,
        "good record must be applied despite sibling failure"
    );

    // 失敗した record は state=2、正常な方は state=1
    let states: Vec<i16> = sqlx::query_scalar(
        "select parse_state from ingest.raw_records where run_id = $1 order by seq",
    )
    .bind(run_id)
    .fetch_all(store.pool())
    .await?;
    assert_eq!(states, vec![2, 1]);

    // parse_errors は run_id 単位で見る (並行テストと混ざらない)
    let errors: i64 =
        sqlx::query_scalar("select count(*) from ingest.parse_errors where run_id = $1")
            .bind(run_id)
            .fetch_one(store.pool())
            .await?;
    assert_eq!(errors, 1);

    // reparse しても parse_errors は raw_record 単位で重複しない
    store.reset_parse_state(false).await?;
    store.normalize_pending().await?;
    let errors: i64 =
        sqlx::query_scalar("select count(*) from ingest.parse_errors where run_id = $1")
            .bind(run_id)
            .fetch_one(store.pool())
            .await?;
    assert_eq!(errors, 1);

    sqlx::query("delete from jv.races where race_key = $1")
        .bind(race_key)
        .execute(store.pool())
        .await?;
    cleanup(&store, ds).await
}

/// 古い raw の再適用 (reparse) で、より新しい raw から適用済みの
/// jv.* 行を巻き戻さない。削除 (data_kubun=0) も tombstone に記録
/// されるので、削除より古い upsert の再適用で復活しない。
#[tokio::test]
async fn reparse_of_old_raw_cannot_regress_entity() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTG";
    let ws = "20050101000000";
    let ketto = "8888888888";
    cleanup(&store, ds).await?;
    sqlx::query("delete from jv.horses where ketto_num = $1")
        .bind(ketto)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from jv.entity_tombstones where entity_key = $1")
        .bind(ketto)
        .execute(store.pool())
        .await?;

    store.seed_window(ds, ws, None, "setup").await?;
    let run_id = store.begin_window(ds, ws, None, "setup", 4).await?;

    async fn bamei(store: &Store, ketto: &str) -> Result<Option<String>, sqlx::Error> {
        sqlx::query_scalar::<_, String>("select bamei from jv.horses where ketto_num = $1")
            .bind(ketto)
            .fetch_optional(store.pool())
            .await
    }

    // UM-A (raw1) → UM-B (raw2, 新しい) の順に適用
    let um_a = bridge::fixture::um(ketto, "ウマエー", "父", "母", "20200101");
    let um_b = bridge::fixture::um(ketto, "ウマビー", "父", "母", "20200101");
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTGFILE01.jvd",
            &[rec(0, 0, "UM", "1", &um_a)],
        )
        .await?;
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTGFILE02.jvd",
            &[rec(1, 0, "UM", "1", &um_b)],
        )
        .await?;
    store.normalize_pending().await?;
    assert_eq!(bamei(&store, ketto).await?.as_deref(), Some("ウマビー"));

    // 古い raw1 を pending に戻して再適用 → ガードで巻き戻らない
    sqlx::query("update ingest.raw_records set parse_state = 0 where seq = 0 and run_id = $1")
        .bind(run_id)
        .execute(store.pool())
        .await?;
    store.normalize_pending().await?;
    assert_eq!(
        bamei(&store, ketto).await?.as_deref(),
        Some("ウマビー"),
        "older raw must not overwrite the newer entity state"
    );

    // raw3 (最新) の data_kubun=0 で削除 → tombstone に記録される
    let mut um_del = um_b.clone();
    um_del[2] = b'0';
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTGFILE03.jvd",
            &[rec(2, 0, "UM", "0", &um_del)],
        )
        .await?;
    store.normalize_pending().await?;
    assert_eq!(
        bamei(&store, ketto).await?,
        None,
        "delete must remove the row"
    );

    // 削除 (raw3) より古い upsert (raw2) の再適用 → 復活しない
    sqlx::query("update ingest.raw_records set parse_state = 0 where seq = 1 and run_id = $1")
        .bind(run_id)
        .execute(store.pool())
        .await?;
    store.normalize_pending().await?;
    assert_eq!(
        bamei(&store, ketto).await?,
        None,
        "older upsert must not resurrect a tombstoned entity"
    );

    // 削除より新しい upsert は普通に適用される
    let um_c = bridge::fixture::um(ketto, "ウマシー", "父", "母", "20200101");
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTGFILE04.jvd",
            &[rec(3, 0, "UM", "1", &um_c)],
        )
        .await?;
    store.normalize_pending().await?;
    assert_eq!(bamei(&store, ketto).await?.as_deref(), Some("ウマシー"));

    sqlx::query("delete from jv.entity_tombstones where entity_key = $1")
        .bind(ketto)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from jv.horses where ketto_num = $1")
        .bind(ketto)
        .execute(store.pool())
        .await?;
    cleanup(&store, ds).await
}

/// HR はレース単位で払戻を置き換える。訂正版で消えた組番は残らず、
/// 古い HR の再適用で新しい適用結果を巻き戻さない。
#[tokio::test]
async fn hr_replaces_race_payouts() -> Result<(), sqlx::Error> {
    let Some(store) = db().await? else {
        return Ok(());
    };
    // DB を共有するテスト同士は advisory lock で直列化する
    let _lock = store.ingest_lock().await?;
    let ds = "TESTH";
    let ws = "20060101000000";
    let race_key = "2030010599010113";
    cleanup(&store, ds).await?;
    sqlx::query("delete from jv.payouts where race_key = $1")
        .bind(race_key)
        .execute(store.pool())
        .await?;
    sqlx::query("delete from jv.entity_tombstones where entity_key = $1")
        .bind(race_key)
        .execute(store.pool())
        .await?;

    store.seed_window(ds, ws, None, "setup").await?;
    let run_id = store.begin_window(ds, ws, None, "setup", 4).await?;

    async fn combos(store: &Store, race_key: &str) -> Result<Vec<String>, sqlx::Error> {
        sqlx::query_scalar("select combo from jv.payouts where race_key = $1 order by combo")
            .bind(race_key)
            .fetch_all(store.pool())
            .await
    }

    // file1: 単勝 3 番
    let hr1 = bridge::fixture::hr("20300105", "99", "01", "01", "13", (3, 250));
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTHFILE01.jvd",
            &[rec(0, 0, "HR", "7", &hr1)],
        )
        .await?;
    store.normalize_pending().await?;
    assert_eq!(combos(&store, race_key).await?, vec!["03".to_string()]);

    // file2 (新しい): 訂正版は組番 07 のみ → 03 は消える
    let mut hr2 = hr1.clone();
    hr2[102] = b'0';
    hr2[103] = b'7';
    store
        .commit_file(
            run_id,
            ds,
            ws,
            "TESTHFILE02.jvd",
            &[rec(1, 0, "HR", "7", &hr2)],
        )
        .await?;
    store.normalize_pending().await?;
    assert_eq!(
        combos(&store, race_key).await?,
        vec!["07".to_string()],
        "newer HR must replace the whole race's payouts"
    );

    // 古い raw1 の再適用 → 巻き戻らない
    sqlx::query("update ingest.raw_records set parse_state = 0 where seq = 0 and run_id = $1")
        .bind(run_id)
        .execute(store.pool())
        .await?;
    store.normalize_pending().await?;
    assert_eq!(combos(&store, race_key).await?, vec!["07".to_string()]);

    sqlx::query("delete from jv.payouts where race_key = $1")
        .bind(race_key)
        .execute(store.pool())
        .await?;
    cleanup(&store, ds).await
}
