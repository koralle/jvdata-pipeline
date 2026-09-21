//! ingest.raw_records → jv.* への正規化。
//!
//! - parse_state=0 の raw を id 順 (= 取得順) に処理する。後から来た
//!   レコードが勝つ (upsert は無条件上書き)。
//! - jv.* の各行は最後に適用した raw id を applied_raw_id に持ち、
//!   それより古い raw の upsert/delete は捨てる。reparse で古い
//!   レコードを回しても適用済みの新しい状態を巻き戻さない。
//! - データ区分=0 (該当レコード削除) のレコードは jv.* 側の対応行を削除する。
//! - 1 レコードの parse/反映失敗は savepoint で切り離して parse_errors に
//!   記録し、バッチ内の他レコードは止めない。
//! - 未対応レコード種別は state=3 (skipped) で明示的にマークする。

use crate::Store;
use jvdata_core::model::*;
use jvdata_core::parse::parse_record;
use jvdata_core::record::{ParsedRecord, SUPPORTED_TYPES};
use sqlx::{Postgres, Transaction};
use tracing::{debug, warn};

const BATCH: i64 = 500;

#[derive(Debug, Default)]
pub struct NormalizeSummary {
    pub processed: u64,
    pub normalized: u64,
    pub parse_errors: u64,
    pub skipped_unknown: u64,
}

enum Outcome {
    Normalized,
    SkippedUnknown,
    Failed(String),
}

impl Store {
    /// pending raw_records をまとめて jv.* へ反映する。
    /// 戻り値の summary は呼び出し全体の合算。
    pub async fn normalize_pending(&self) -> Result<NormalizeSummary, sqlx::Error> {
        let mut total = NormalizeSummary::default();
        loop {
            let batch = sqlx::query_as::<_, PendingRow>(
                "select id, run_id, filename, record_type, payload
                   from ingest.raw_records
                  where parse_state = 0
                  order by id
                  limit $1",
            )
            .bind(BATCH)
            .fetch_all(self.pool())
            .await?;
            if batch.is_empty() {
                return Ok(total);
            }
            let n = batch.len();
            let mut tx = self.pool().begin().await?;
            for row in batch {
                total.processed += 1;
                // 1 レコード = 1 savepoint。反映中の SQL エラーでバッチ全体を
                // 巻き戻さず、そのレコードだけ parse_errors へ記録して進める。
                sqlx::query("savepoint rec").execute(&mut *tx).await?;
                match process_record(&mut tx, &row).await {
                    Ok(Outcome::Normalized) => {
                        sqlx::query("release savepoint rec")
                            .execute(&mut *tx)
                            .await?;
                        total.normalized += 1;
                    }
                    Ok(Outcome::SkippedUnknown) => {
                        sqlx::query("release savepoint rec")
                            .execute(&mut *tx)
                            .await?;
                        total.skipped_unknown += 1;
                    }
                    Ok(Outcome::Failed(msg)) => {
                        sqlx::query("rollback to savepoint rec")
                            .execute(&mut *tx)
                            .await?;
                        sqlx::query("release savepoint rec")
                            .execute(&mut *tx)
                            .await?;
                        warn!(id = row.id, r#type = %row.record_type, error = %msg, "parse error");
                        record_failure(&mut tx, &row, &msg).await?;
                        total.parse_errors += 1;
                    }
                    Err(e) => {
                        // 反映中の DB エラー。savepoint まで巻き戻せば他レコードは
                        // 生かせる。巻き戻せない (接続断等) なら batch ごと失敗。
                        if sqlx::query("rollback to savepoint rec")
                            .execute(&mut *tx)
                            .await
                            .is_err()
                        {
                            return Err(e);
                        }
                        sqlx::query("release savepoint rec")
                            .execute(&mut *tx)
                            .await?;
                        warn!(id = row.id, r#type = %row.record_type, error = %e, "record apply error");
                        record_failure(&mut tx, &row, &e.to_string()).await?;
                        total.parse_errors += 1;
                    }
                }
            }
            debug!(batch = n, "normalize batch committed");
            tx.commit().await?;
        }
    }

    /// parse 失敗 (2) / 未対応種別 (3) を parse_state=0 に戻して再処理可能にする。
    /// parser を修正したあと `jvdata reparse` で使う。all=true なら normalized (1)
    /// を含む全レコードを対象にする (upsert は冪等なので再適用しても安全)。
    /// 戻り値は戻した行数。
    pub async fn reset_parse_state(&self, all: bool) -> Result<u64, sqlx::Error> {
        let r = if all {
            sqlx::query("update ingest.raw_records set parse_state = 0")
                .execute(self.pool())
                .await?
        } else {
            sqlx::query("update ingest.raw_records set parse_state = 0 where parse_state in (2, 3)")
                .execute(self.pool())
                .await?
        };
        Ok(r.rows_affected())
    }
}

#[derive(sqlx::FromRow)]
struct PendingRow {
    id: i64,
    run_id: i64,
    filename: String,
    record_type: String,
    payload: Vec<u8>,
}

async fn set_state(
    tx: &mut Transaction<'_, Postgres>,
    id: i64,
    state: i16,
) -> Result<(), sqlx::Error> {
    sqlx::query("update ingest.raw_records set parse_state = $1 where id = $2")
        .bind(state)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// 1 raw record の処理。parse 失敗は Ok(Failed) で返し、DB 書き込みは
/// 行わない (呼び出し側が savepoint ごと巻き戻してから失敗を記録する)。
async fn process_record(
    tx: &mut Transaction<'_, Postgres>,
    row: &PendingRow,
) -> Result<Outcome, sqlx::Error> {
    if !SUPPORTED_TYPES.contains(&row.record_type.as_str()) {
        set_state(tx, row.id, 3).await?;
        return Ok(Outcome::SkippedUnknown);
    }
    match parse_record(&row.payload) {
        Ok(rec) => {
            apply(tx, &rec, &row.filename, row.id).await?;
            set_state(tx, row.id, 1).await?;
            Ok(Outcome::Normalized)
        }
        Err(e) => Ok(Outcome::Failed(e.to_string())),
    }
}

/// 失敗レコードを parse_errors + runs.parse_errors + parse_state=2 で記録する。
/// parse_errors は raw_record_id 単位で upsert (reparse で重複行を増やさない)。
async fn record_failure(
    tx: &mut Transaction<'_, Postgres>,
    row: &PendingRow,
    msg: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into ingest.parse_errors
            (raw_record_id, run_id, record_type, filename, error)
         values ($1, $2, $3, $4, $5)
         on conflict (raw_record_id) do update set
            run_id = excluded.run_id,
            record_type = excluded.record_type,
            filename = excluded.filename,
            error = excluded.error,
            created_at = now()",
    )
    .bind(row.id)
    .bind(row.run_id)
    .bind(&row.record_type)
    .bind(&row.filename)
    .bind(msg)
    .execute(&mut **tx)
    .await?;
    // parse_errors 表は raw_record 単位で dedupe されるので、counter も
    // reparse で膨らまないよう実件数に合わせる。
    sqlx::query(
        "update ingest.runs
            set parse_errors = (select count(*) from ingest.parse_errors
                                 where run_id = $1)
          where id = $1",
    )
    .bind(row.run_id)
    .execute(&mut **tx)
    .await?;
    set_state(tx, row.id, 2).await
}

/// レコードが触る jv.* エンティティとそのキー (tombstone のキー)。
/// payouts は 1 レース全件を 1 単位で置き換えるので race_key 単位。
fn entity_key(rec: &ParsedRecord) -> Option<(&'static str, String)> {
    match rec {
        ParsedRecord::Ra(r) => Some(("races", r.race_key.clone())),
        ParsedRecord::Se(r) => Some(("entries", format!("{}/{}", r.race_key, r.umaban))),
        ParsedRecord::Hr(r) => Some(("payouts", r.race_key.clone())),
        ParsedRecord::Um(r) => Some(("horses", r.ketto_num.clone())),
        ParsedRecord::Ks(r) => Some(("jockeys", r.kisyu_code.clone())),
        ParsedRecord::Ch(r) => Some(("trainers", r.chokyosi_code.clone())),
        ParsedRecord::Ys(r) => r
            .race_date
            .map(|d| ("schedule_days", format!("{d}/{}", r.jyo_cd))),
    }
}

/// データ区分=0 は「該当レコード削除」(JV-Data 仕様書: 提供ミス等による
/// 取消し)。upsert ではなく jv.* 側の対応行の削除として適用する。
/// `raw_id` は適用の版数 (upsert/delete の新旧判定に使う)。
async fn apply(
    tx: &mut Transaction<'_, Postgres>,
    rec: &ParsedRecord,
    source_file: &str,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    // 削除済みエンティティを、それより古い raw で復活させない。
    // tombstone に記録された削除より古い upsert/delete は両方捨てる。
    if let Some((entity, key)) = entity_key(rec) {
        let tomb: Option<i64> = sqlx::query_scalar(
            "select applied_raw_id from jv.entity_tombstones
              where entity = $1 and entity_key = $2",
        )
        .bind(entity)
        .bind(&key)
        .fetch_optional(&mut **tx)
        .await?;
        if tomb.is_some_and(|t| t >= raw_id) {
            return Ok(());
        }
    }
    if rec.head().data_kubun == "0" {
        return delete_record(tx, rec, raw_id).await;
    }
    match rec {
        ParsedRecord::Ra(r) => upsert_race(tx, r, source_file, raw_id).await,
        ParsedRecord::Se(r) => upsert_entry(tx, r, source_file, raw_id).await,
        ParsedRecord::Hr(r) => upsert_payouts(tx, r, raw_id).await,
        ParsedRecord::Um(r) => upsert_horse(tx, r, raw_id).await,
        ParsedRecord::Ks(r) => upsert_jockey(tx, r, raw_id).await,
        ParsedRecord::Ch(r) => upsert_trainer(tx, r, raw_id).await,
        ParsedRecord::Ys(r) => upsert_schedule(tx, r, source_file, raw_id).await,
    }
}

/// データ区分=0 の削除。レコードのキー項目だけを使って対応行を消す
/// (HR は 1 レコード = そのレースの払戻全件なので race_key で消す)。
/// 適用済みの行がこの delete より新しい raw から来ている場合は
/// 巻き戻さない (applied_raw_id ガード)。削除は行が無くても
/// tombstone に記録する: 後から来る古い upsert の復活を防ぐため。
async fn delete_record(
    tx: &mut Transaction<'_, Postgres>,
    rec: &ParsedRecord,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    let Some((entity, key)) = entity_key(rec) else {
        return Ok(());
    };
    match rec {
        ParsedRecord::Ra(r) => {
            sqlx::query(
                "delete from jv.races
                  where race_key = $1
                    and (applied_raw_id is null or applied_raw_id < $2)",
            )
            .bind(&r.race_key)
            .bind(raw_id)
            .execute(&mut **tx)
            .await?;
        }
        ParsedRecord::Se(r) => {
            sqlx::query(
                "delete from jv.entries
                  where race_key = $1 and umaban = $2
                    and (applied_raw_id is null or applied_raw_id < $3)",
            )
            .bind(&r.race_key)
            .bind(&r.umaban)
            .bind(raw_id)
            .execute(&mut **tx)
            .await?;
        }
        ParsedRecord::Hr(r) => {
            sqlx::query(
                "delete from jv.payouts
                  where race_key = $1
                    and (applied_raw_id is null or applied_raw_id < $2)",
            )
            .bind(&r.race_key)
            .bind(raw_id)
            .execute(&mut **tx)
            .await?;
        }
        ParsedRecord::Um(r) => {
            sqlx::query(
                "delete from jv.horses
                  where ketto_num = $1
                    and (applied_raw_id is null or applied_raw_id < $2)",
            )
            .bind(&r.ketto_num)
            .bind(raw_id)
            .execute(&mut **tx)
            .await?;
        }
        ParsedRecord::Ks(r) => {
            sqlx::query(
                "delete from jv.jockeys
                  where kisyu_code = $1
                    and (applied_raw_id is null or applied_raw_id < $2)",
            )
            .bind(&r.kisyu_code)
            .bind(raw_id)
            .execute(&mut **tx)
            .await?;
        }
        ParsedRecord::Ch(r) => {
            sqlx::query(
                "delete from jv.trainers
                  where chokyosi_code = $1
                    and (applied_raw_id is null or applied_raw_id < $2)",
            )
            .bind(&r.chokyosi_code)
            .bind(raw_id)
            .execute(&mut **tx)
            .await?;
        }
        ParsedRecord::Ys(r) => {
            if let Some(d) = r.race_date {
                sqlx::query(
                    "delete from jv.schedule_days
                      where race_date = $1 and jyo_cd = $2
                        and (applied_raw_id is null or applied_raw_id < $3)",
                )
                .bind(d)
                .bind(&r.jyo_cd)
                .bind(raw_id)
                .execute(&mut **tx)
                .await?;
            }
        }
    }
    sqlx::query(
        "insert into jv.entity_tombstones (entity, entity_key, applied_raw_id)
         values ($1, $2, $3)
         on conflict (entity, entity_key) do update
           set applied_raw_id = excluded.applied_raw_id,
               applied_at = now()
           where excluded.applied_raw_id
               > jv.entity_tombstones.applied_raw_id",
    )
    .bind(entity)
    .bind(key)
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_race(
    tx: &mut Transaction<'_, Postgres>,
    r: &RaRace,
    source_file: &str,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into jv.races (
            race_key, race_date, jyo_cd, kaiji, nichiji, race_num, youbi_cd,
            toku_num, hondai, fukudai, kakko, ryakusyo10, ryakusyo6, ryakusyo3,
            grade_cd, syubetu_cd, kigo_cd, jyuryo_cd, jyoken_name, kyori,
            track_cd, course_kubun_cd, hasso_time, toroku_tosu, syusso_tosu,
            nyusen_tosu, tenko_cd, siba_baba_cd, dirt_baba_cd,
            data_kubun, make_date, source_file, applied_raw_id)
         values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,
                 $19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,$33)
         on conflict (race_key) do update set
            race_date = excluded.race_date, jyo_cd = excluded.jyo_cd,
            kaiji = excluded.kaiji, nichiji = excluded.nichiji,
            race_num = excluded.race_num, youbi_cd = excluded.youbi_cd,
            toku_num = excluded.toku_num, hondai = excluded.hondai,
            fukudai = excluded.fukudai, kakko = excluded.kakko,
            ryakusyo10 = excluded.ryakusyo10, ryakusyo6 = excluded.ryakusyo6,
            ryakusyo3 = excluded.ryakusyo3, grade_cd = excluded.grade_cd,
            syubetu_cd = excluded.syubetu_cd, kigo_cd = excluded.kigo_cd,
            jyuryo_cd = excluded.jyuryo_cd, jyoken_name = excluded.jyoken_name,
            kyori = excluded.kyori, track_cd = excluded.track_cd,
            course_kubun_cd = excluded.course_kubun_cd,
            hasso_time = excluded.hasso_time, toroku_tosu = excluded.toroku_tosu,
            syusso_tosu = excluded.syusso_tosu, nyusen_tosu = excluded.nyusen_tosu,
            tenko_cd = excluded.tenko_cd, siba_baba_cd = excluded.siba_baba_cd,
            dirt_baba_cd = excluded.dirt_baba_cd,
            data_kubun = excluded.data_kubun, make_date = excluded.make_date,
            source_file = excluded.source_file,
            applied_raw_id = excluded.applied_raw_id, updated_at = now()
         where jv.races.applied_raw_id is null
            or excluded.applied_raw_id > jv.races.applied_raw_id",
    )
    .bind(&r.race_key)
    .bind(r.race_date)
    .bind(&r.jyo_cd)
    .bind(r.kaiji.map(|v| v as i16))
    .bind(r.nichiji.map(|v| v as i16))
    .bind(r.race_num.map(|v| v as i16))
    .bind(&r.youbi_cd)
    .bind(r.toku_num.map(|v| v as i32))
    .bind(&r.hondai)
    .bind(&r.fukudai)
    .bind(&r.kakko)
    .bind(&r.ryakusyo10)
    .bind(&r.ryakusyo6)
    .bind(&r.ryakusyo3)
    .bind(&r.grade_cd)
    .bind(&r.syubetu_cd)
    .bind(&r.kigo_cd)
    .bind(&r.jyuryo_cd)
    .bind(&r.jyoken_name)
    .bind(r.kyori.map(|v| v as i32))
    .bind(&r.track_cd)
    .bind(&r.course_kubun_cd)
    .bind(&r.hasso_time)
    .bind(r.toroku_tosu.map(|v| v as i16))
    .bind(r.syusso_tosu.map(|v| v as i16))
    .bind(r.nyusen_tosu.map(|v| v as i16))
    .bind(&r.tenko_cd)
    .bind(&r.siba_baba_cd)
    .bind(&r.dirt_baba_cd)
    .bind(&r.head.data_kubun)
    .bind(r.head.make_date)
    .bind(source_file)
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_entry(
    tx: &mut Transaction<'_, Postgres>,
    r: &SeRaceUma,
    source_file: &str,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into jv.entries (
            race_key, umaban, wakuban, ketto_num, bamei, uma_kigo_cd, sex_cd,
            hinsyu_cd, keiro_cd, barei, tozai_cd, chokyosi_code,
            chokyosi_ryakusyo, banusi_code, banusi_name, futan, blinker,
            kisyu_code, kisyu_ryakusyo, minarai_cd, ba_taijyu, zogen_sa,
            ijyo_cd, nyusen_jyuni, kakutei_jyuni, dochaku_kubun, time_sec,
            chakusa_cd, jyuni_1c, jyuni_2c, jyuni_3c, jyuni_4c, odds, ninki,
            honsyokin, fukasyokin, haron_l3_sec, data_kubun, make_date,
            source_file, zogen_fugo, applied_raw_id)
         values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
                 $18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28,$29,$30,$31,$32,
                 $33,$34,$35,$36,$37,$38,$39,$40,$41,$42)
         on conflict (race_key, umaban) do update set
            wakuban = excluded.wakuban, ketto_num = excluded.ketto_num,
            bamei = excluded.bamei, uma_kigo_cd = excluded.uma_kigo_cd,
            sex_cd = excluded.sex_cd, hinsyu_cd = excluded.hinsyu_cd,
            keiro_cd = excluded.keiro_cd, barei = excluded.barei,
            tozai_cd = excluded.tozai_cd, chokyosi_code = excluded.chokyosi_code,
            chokyosi_ryakusyo = excluded.chokyosi_ryakusyo,
            banusi_code = excluded.banusi_code, banusi_name = excluded.banusi_name,
            futan = excluded.futan, blinker = excluded.blinker,
            kisyu_code = excluded.kisyu_code, kisyu_ryakusyo = excluded.kisyu_ryakusyo,
            minarai_cd = excluded.minarai_cd, ba_taijyu = excluded.ba_taijyu,
            zogen_sa = excluded.zogen_sa, ijyo_cd = excluded.ijyo_cd,
            nyusen_jyuni = excluded.nyusen_jyuni,
            kakutei_jyuni = excluded.kakutei_jyuni,
            dochaku_kubun = excluded.dochaku_kubun, time_sec = excluded.time_sec,
            chakusa_cd = excluded.chakusa_cd, jyuni_1c = excluded.jyuni_1c,
            jyuni_2c = excluded.jyuni_2c, jyuni_3c = excluded.jyuni_3c,
            jyuni_4c = excluded.jyuni_4c, odds = excluded.odds,
            ninki = excluded.ninki, honsyokin = excluded.honsyokin,
            fukasyokin = excluded.fukasyokin, haron_l3_sec = excluded.haron_l3_sec,
            data_kubun = excluded.data_kubun, make_date = excluded.make_date,
            source_file = excluded.source_file, zogen_fugo = excluded.zogen_fugo,
            applied_raw_id = excluded.applied_raw_id, updated_at = now()
         where jv.entries.applied_raw_id is null
            or excluded.applied_raw_id > jv.entries.applied_raw_id",
    )
    .bind(&r.race_key)
    .bind(&r.umaban)
    .bind(r.wakuban.map(|v| v as i16))
    .bind(&r.ketto_num)
    .bind(&r.bamei)
    .bind(&r.uma_kigo_cd)
    .bind(&r.sex_cd)
    .bind(&r.hinsyu_cd)
    .bind(&r.keiro_cd)
    .bind(r.barei.map(|v| v as i16))
    .bind(&r.tozai_cd)
    .bind(&r.chokyosi_code)
    .bind(&r.chokyosi_ryakusyo)
    .bind(&r.banusi_code)
    .bind(&r.banusi_name)
    .bind(r.futan)
    .bind(&r.blinker)
    .bind(&r.kisyu_code)
    .bind(&r.kisyu_ryakusyo)
    .bind(&r.minarai_cd)
    .bind(r.ba_taijyu.map(|v| v as i16))
    .bind(r.zogen_sa.map(|v| v as i16))
    .bind(&r.ijyo_cd)
    .bind(r.nyusen_jyuni.map(|v| v as i16))
    .bind(r.kakutei_jyuni.map(|v| v as i16))
    .bind(&r.dochaku_kubun)
    .bind(r.time_sec)
    .bind(&r.chakusa_cd)
    .bind(r.jyuni_1c.map(|v| v as i16))
    .bind(r.jyuni_2c.map(|v| v as i16))
    .bind(r.jyuni_3c.map(|v| v as i16))
    .bind(r.jyuni_4c.map(|v| v as i16))
    .bind(r.odds)
    .bind(r.ninki.map(|v| v as i16))
    .bind(r.honsyokin)
    .bind(r.fukasyokin)
    .bind(r.haron_l3_sec)
    .bind(&r.head.data_kubun)
    .bind(r.head.make_date)
    .bind(source_file)
    .bind(&r.zogen_fugo)
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_payouts(
    tx: &mut Transaction<'_, Postgres>,
    r: &HrPay,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    // HR は 1 レコード = そのレースの払戻全件。行ごとの upsert だと
    // 訂正版で消えた組番が残ってしまうので、レース単位で置き換える。
    // また reparse 等で古い HR が来たとき新しい適用結果を巻き戻さない
    // よう、そのレースに適用済みの最新 raw id と比較する。
    let applied: Option<i64> =
        sqlx::query_scalar("select max(applied_raw_id) from jv.payouts where race_key = $1")
            .bind(&r.race_key)
            .fetch_one(&mut **tx)
            .await?;
    if applied.is_some_and(|a| a >= raw_id) {
        return Ok(());
    }
    sqlx::query("delete from jv.payouts where race_key = $1")
        .bind(&r.race_key)
        .execute(&mut **tx)
        .await?;
    for p in &r.payouts {
        sqlx::query(
            "insert into jv.payouts (race_key, bet_type, combo, pay, ninki, applied_raw_id)
             values ($1, $2, $3, $4, $5, $6)
             on conflict (race_key, bet_type, combo) do update set
                pay = excluded.pay, ninki = excluded.ninki,
                applied_raw_id = excluded.applied_raw_id",
        )
        .bind(&r.race_key)
        .bind(p.bet_type)
        .bind(&p.combo)
        .bind(p.pay as i32)
        .bind(p.ninki.map(|v| v as i32))
        .bind(raw_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn upsert_horse(
    tx: &mut Transaction<'_, Postgres>,
    r: &UmUma,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into jv.horses (
            ketto_num, del_kubun, reg_date, del_date, birth_date, bamei,
            bamei_kana, bamei_eng, sex_cd, hinsyu_cd, keiro_cd, sire_name,
            dam_name, tozai_cd, chokyosi_code, chokyosi_ryakusyo, breeder_code,
            breeder_name, sanchi_name, banusi_code, banusi_name, race_count,
            applied_raw_id)
         values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,
                 $18,$19,$20,$21,$22,$23)
         on conflict (ketto_num) do update set
            del_kubun = excluded.del_kubun, reg_date = excluded.reg_date,
            del_date = excluded.del_date, birth_date = excluded.birth_date,
            bamei = excluded.bamei, bamei_kana = excluded.bamei_kana,
            bamei_eng = excluded.bamei_eng, sex_cd = excluded.sex_cd,
            hinsyu_cd = excluded.hinsyu_cd, keiro_cd = excluded.keiro_cd,
            sire_name = excluded.sire_name, dam_name = excluded.dam_name,
            tozai_cd = excluded.tozai_cd, chokyosi_code = excluded.chokyosi_code,
            chokyosi_ryakusyo = excluded.chokyosi_ryakusyo,
            breeder_code = excluded.breeder_code,
            breeder_name = excluded.breeder_name,
            sanchi_name = excluded.sanchi_name,
            banusi_code = excluded.banusi_code,
            banusi_name = excluded.banusi_name,
            race_count = excluded.race_count,
            applied_raw_id = excluded.applied_raw_id, updated_at = now()
         where jv.horses.applied_raw_id is null
            or excluded.applied_raw_id > jv.horses.applied_raw_id",
    )
    .bind(&r.ketto_num)
    .bind(&r.del_kubun)
    .bind(r.reg_date)
    .bind(r.del_date)
    .bind(r.birth_date)
    .bind(&r.bamei)
    .bind(&r.bamei_kana)
    .bind(&r.bamei_eng)
    .bind(&r.sex_cd)
    .bind(&r.hinsyu_cd)
    .bind(&r.keiro_cd)
    .bind(&r.sire_name)
    .bind(&r.dam_name)
    .bind(&r.tozai_cd)
    .bind(&r.chokyosi_code)
    .bind(&r.chokyosi_ryakusyo)
    .bind(&r.breeder_code)
    .bind(&r.breeder_name)
    .bind(&r.sanchi_name)
    .bind(&r.banusi_code)
    .bind(&r.banusi_name)
    .bind(r.race_count.map(|v| v as i32))
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_jockey(
    tx: &mut Transaction<'_, Postgres>,
    r: &KsKisyu,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into jv.jockeys (
            kisyu_code, del_kubun, kisyu_name, kisyu_name_kana, kisyu_ryakusyo,
            kisyu_name_eng, sex_cd, minarai_cd, tozai_cd, chokyosi_code,
            applied_raw_id)
         values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
         on conflict (kisyu_code) do update set
            del_kubun = excluded.del_kubun, kisyu_name = excluded.kisyu_name,
            kisyu_name_kana = excluded.kisyu_name_kana,
            kisyu_ryakusyo = excluded.kisyu_ryakusyo,
            kisyu_name_eng = excluded.kisyu_name_eng, sex_cd = excluded.sex_cd,
            minarai_cd = excluded.minarai_cd, tozai_cd = excluded.tozai_cd,
            chokyosi_code = excluded.chokyosi_code,
            applied_raw_id = excluded.applied_raw_id, updated_at = now()
         where jv.jockeys.applied_raw_id is null
            or excluded.applied_raw_id > jv.jockeys.applied_raw_id",
    )
    .bind(&r.kisyu_code)
    .bind(&r.del_kubun)
    .bind(&r.kisyu_name)
    .bind(&r.kisyu_name_kana)
    .bind(&r.kisyu_ryakusyo)
    .bind(&r.kisyu_name_eng)
    .bind(&r.sex_cd)
    .bind(&r.minarai_cd)
    .bind(&r.tozai_cd)
    .bind(&r.chokyosi_code)
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_trainer(
    tx: &mut Transaction<'_, Postgres>,
    r: &ChChokyosi,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into jv.trainers (
            chokyosi_code, del_kubun, chokyosi_name, chokyosi_name_kana,
            chokyosi_ryakusyo, sex_cd, tozai_cd, applied_raw_id)
         values ($1,$2,$3,$4,$5,$6,$7,$8)
         on conflict (chokyosi_code) do update set
            del_kubun = excluded.del_kubun, chokyosi_name = excluded.chokyosi_name,
            chokyosi_name_kana = excluded.chokyosi_name_kana,
            chokyosi_ryakusyo = excluded.chokyosi_ryakusyo,
            sex_cd = excluded.sex_cd, tozai_cd = excluded.tozai_cd,
            applied_raw_id = excluded.applied_raw_id, updated_at = now()
         where jv.trainers.applied_raw_id is null
            or excluded.applied_raw_id > jv.trainers.applied_raw_id",
    )
    .bind(&r.chokyosi_code)
    .bind(&r.del_kubun)
    .bind(&r.chokyosi_name)
    .bind(&r.chokyosi_name_kana)
    .bind(&r.chokyosi_ryakusyo)
    .bind(&r.sex_cd)
    .bind(&r.tozai_cd)
    .bind(raw_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn upsert_schedule(
    tx: &mut Transaction<'_, Postgres>,
    r: &YsSchedule,
    _source_file: &str,
    raw_id: i64,
) -> Result<(), sqlx::Error> {
    if let Some(d) = r.race_date {
        sqlx::query(
            "insert into jv.schedule_days (race_date, jyo_cd, kaiji, nichiji, youbi_cd, applied_raw_id)
             values ($1, $2, $3, $4, $5, $6)
             on conflict (race_date, jyo_cd) do update set
                kaiji = excluded.kaiji, nichiji = excluded.nichiji,
                youbi_cd = excluded.youbi_cd,
                applied_raw_id = excluded.applied_raw_id, updated_at = now()
             where jv.schedule_days.applied_raw_id is null
                or excluded.applied_raw_id > jv.schedule_days.applied_raw_id",
        )
        .bind(d)
        .bind(&r.jyo_cd)
        .bind(r.kaiji.map(|v| v as i16))
        .bind(r.nichiji.map(|v| v as i16))
        .bind(&r.youbi_cd)
        .bind(raw_id)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
