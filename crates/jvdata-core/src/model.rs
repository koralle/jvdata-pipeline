//! 正規化先 `jv.*` に対応する parse 済みモデル。
//!
//! フィールド名は JV-Data 仕様書の項目名をそのまま使う。

use crate::record::RecordHead;
use time::Date;

/// RA レース詳細 → jv.races
#[derive(Debug)]
pub struct RaRace {
    pub head: RecordHead,
    pub race_key: String,
    pub race_date: Option<Date>,
    pub jyo_cd: String,
    pub kaiji: Option<i64>,
    pub nichiji: Option<i64>,
    pub race_num: Option<i64>,
    pub youbi_cd: Option<String>,
    pub toku_num: Option<i64>,
    pub hondai: Option<String>,
    pub fukudai: Option<String>,
    pub kakko: Option<String>,
    pub ryakusyo10: Option<String>,
    pub ryakusyo6: Option<String>,
    pub ryakusyo3: Option<String>,
    pub grade_cd: Option<String>,
    pub syubetu_cd: Option<String>,
    pub kigo_cd: Option<String>,
    pub jyuryo_cd: Option<String>,
    pub jyoken_name: Option<String>,
    pub kyori: Option<i64>,
    pub track_cd: Option<String>,
    pub course_kubun_cd: Option<String>,
    pub hasso_time: Option<String>,
    pub toroku_tosu: Option<i64>,
    pub syusso_tosu: Option<i64>,
    pub nyusen_tosu: Option<i64>,
    pub tenko_cd: Option<String>,
    pub siba_baba_cd: Option<String>,
    pub dirt_baba_cd: Option<String>,
}

/// SE 馬毎レース情報 → jv.entries
#[derive(Debug)]
pub struct SeRaceUma {
    pub head: RecordHead,
    pub race_key: String,
    pub wakuban: Option<i64>,
    pub umaban: String,
    pub ketto_num: String,
    pub bamei: Option<String>,
    pub uma_kigo_cd: Option<String>,
    pub sex_cd: Option<String>,
    pub hinsyu_cd: Option<String>,
    pub keiro_cd: Option<String>,
    pub barei: Option<i64>,
    pub tozai_cd: Option<String>,
    pub chokyosi_code: Option<String>,
    pub chokyosi_ryakusyo: Option<String>,
    pub banusi_code: Option<String>,
    pub banusi_name: Option<String>,
    /// 負担重量 (kg。元データは ×10 の整数)
    pub futan: Option<f64>,
    pub blinker: Option<String>,
    pub kisyu_code: Option<String>,
    pub kisyu_ryakusyo: Option<String>,
    pub minarai_cd: Option<String>,
    /// 馬体重 (kg)
    pub ba_taijyu: Option<i64>,
    /// 増減差 (kg、符号別フィールド)
    pub zogen_fugo: Option<String>,
    pub zogen_sa: Option<i64>,
    pub ijyo_cd: Option<String>,
    pub nyusen_jyuni: Option<i64>,
    pub kakutei_jyuni: Option<i64>,
    pub dochaku_kubun: Option<String>,
    /// 走破タイム (秒)。元データは "9分99秒9" の packed 形式
    /// (例: "1335" = 1'33"5 = 93.5 秒)。取消・中止等では None。
    pub time_sec: Option<f64>,
    pub chakusa_cd: Option<String>,
    pub jyuni_1c: Option<i64>,
    pub jyuni_2c: Option<i64>,
    pub jyuni_3c: Option<i64>,
    pub jyuni_4c: Option<i64>,
    /// 単勝オッズ (元データは ×10 の整数)
    pub odds: Option<f64>,
    pub ninki: Option<i64>,
    pub honsyokin: Option<i64>,
    pub fukasyokin: Option<i64>,
    /// 後 3 ハロンタイム (秒。元データは "99秒9" の ×10 整数)
    pub haron_l3_sec: Option<f64>,
}

/// HR 払戻 → jv.payouts (1 行 = 1 券種 1 組番)
#[derive(Debug)]
pub struct HrPay {
    pub head: RecordHead,
    pub race_key: String,
    pub payouts: Vec<Payout>,
}

#[derive(Debug)]
pub struct Payout {
    /// "tansho" | "fukusyo" | "wakuren" | "umaren" | "wide" | "umatan" | "sanrenpuku" | "sanrentan"
    pub bet_type: &'static str,
    /// 馬番 or 組番 (そのままの文字列)
    pub combo: String,
    /// 払戻金 (円)
    pub pay: i64,
    pub ninki: Option<i64>,
}

/// UM 競走馬マスタ → jv.horses
#[derive(Debug)]
pub struct UmUma {
    pub head: RecordHead,
    pub ketto_num: String,
    pub del_kubun: Option<String>,
    pub reg_date: Option<Date>,
    pub del_date: Option<Date>,
    pub birth_date: Option<Date>,
    pub bamei: Option<String>,
    pub bamei_kana: Option<String>,
    pub bamei_eng: Option<String>,
    pub sex_cd: Option<String>,
    pub hinsyu_cd: Option<String>,
    pub keiro_cd: Option<String>,
    /// 3代血統: 父・母・父父・父母・母父・母母… (繁殖登録番号+馬名、14 組)
    pub sire_name: Option<String>,
    pub dam_name: Option<String>,
    pub tozai_cd: Option<String>,
    pub chokyosi_code: Option<String>,
    pub chokyosi_ryakusyo: Option<String>,
    pub breeder_code: Option<String>,
    pub breeder_name: Option<String>,
    pub sanchi_name: Option<String>,
    pub banusi_code: Option<String>,
    pub banusi_name: Option<String>,
    pub race_count: Option<i64>,
}

/// KS 騎手マスタ → jv.jockeys
#[derive(Debug)]
pub struct KsKisyu {
    pub head: RecordHead,
    pub kisyu_code: String,
    pub del_kubun: Option<String>,
    pub kisyu_name: Option<String>,
    pub kisyu_name_kana: Option<String>,
    pub kisyu_ryakusyo: Option<String>,
    pub kisyu_name_eng: Option<String>,
    pub sex_cd: Option<String>,
    pub minarai_cd: Option<String>,
    pub tozai_cd: Option<String>,
    pub chokyosi_code: Option<String>,
}

/// CH 調教師マスタ → jv.trainers
#[derive(Debug)]
pub struct ChChokyosi {
    pub head: RecordHead,
    pub chokyosi_code: String,
    pub del_kubun: Option<String>,
    pub chokyosi_name: Option<String>,
    pub chokyosi_name_kana: Option<String>,
    pub chokyosi_ryakusyo: Option<String>,
    pub sex_cd: Option<String>,
    pub tozai_cd: Option<String>,
}

/// YS 開催スケジュール → jv.schedule_days
#[derive(Debug)]
pub struct YsSchedule {
    pub head: RecordHead,
    pub race_date: Option<Date>,
    pub jyo_cd: String,
    pub kaiji: Option<i64>,
    pub nichiji: Option<i64>,
    pub youbi_cd: Option<String>,
    /// 重賞案内 (最大3件)
    pub jyusyo: Vec<YsJyusyo>,
}

#[derive(Debug)]
pub struct YsJyusyo {
    pub toku_num: Option<i64>,
    pub hondai: Option<String>,
    pub ryakusyo10: Option<String>,
    pub grade_cd: Option<String>,
    pub kyori: Option<i64>,
    pub track_cd: Option<String>,
}
