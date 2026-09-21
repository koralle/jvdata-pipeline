//! raw record の先頭 11 byte (共通ヘッダ) と parse 結果の enum。
//!
//! レイアウトは JV-Data 仕様書 (JV-Data4901) / SDK の
//! JVData_Structure.h に基づく。全フィールドは byte offset。

use crate::ParseError;
use crate::field;
use crate::model::{ChChokyosi, HrPay, KsKisyu, RaRace, SeRaceUma, UmUma, YsSchedule};
use time::Date;

/// `_RECORD_ID`: RecordSpec[2] + DataKubun[1] + MakeDate(YYYYMMDD)[8]
#[derive(Debug, Clone)]
pub struct RecordHead {
    /// "RA" 等のレコード種別 (先頭 2 byte)
    pub record_type: String,
    /// データ区分 (蓄積系データの更新種別)
    pub data_kubun: String,
    /// データ作成年月日
    pub make_date: Option<Date>,
}

impl RecordHead {
    pub fn parse(d: &[u8]) -> Result<Self, ParseError> {
        Ok(Self {
            record_type: field::text(d, 0, 2)?,
            data_kubun: field::text(d, 2, 1)?,
            make_date: field::date(d, 3)?,
        })
    }
}

/// `_RACE_ID` (16 byte, offset 11): Year+MonthDay+JyoCD+Kaiji+Nichiji+RaceNum。
/// レースを一意に識別するキー。そのまま jv.races.race_key に使う。
pub fn race_key(d: &[u8]) -> Result<String, ParseError> {
    field::text(d, 11, 16)
}

/// `_RACE_ID2` (14 byte, offset 11): Year+MonthDay+JyoCD+Kaiji+Nichiji。
/// 開催日単位のキー (YS レコード用)。
pub fn race_id2(d: &[u8]) -> Result<String, ParseError> {
    field::text(d, 11, 14)
}

/// 解析に対応しているレコード種別。
pub const SUPPORTED_TYPES: &[&str] = &["RA", "SE", "HR", "UM", "KS", "CH", "YS"];

#[derive(Debug)]
pub enum ParsedRecord {
    Ra(RaRace),
    Se(SeRaceUma),
    Hr(HrPay),
    Um(UmUma),
    Ks(KsKisyu),
    Ch(ChChokyosi),
    Ys(YsSchedule),
}

impl ParsedRecord {
    /// 共通ヘッダ (record_type / data_kubun / make_date)。
    pub fn head(&self) -> &RecordHead {
        match self {
            Self::Ra(r) => &r.head,
            Self::Se(r) => &r.head,
            Self::Hr(r) => &r.head,
            Self::Um(r) => &r.head,
            Self::Ks(r) => &r.head,
            Self::Ch(r) => &r.head,
            Self::Ys(r) => &r.head,
        }
    }
}
