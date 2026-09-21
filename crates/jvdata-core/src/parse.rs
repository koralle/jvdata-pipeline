//! JV-Data raw record → モデルの parse。
//!
//! offset は全て JVData_Structure.h (SDK Ver5.0.0) の struct 定義から
//! 機械的に算出した byte offset。CRLF(2 byte) もレコードに含まれる。

use crate::ParseError;
use crate::field::{date, num, packed_time, raw, scaled10, text, text_opt};
use crate::model::*;
use crate::record::{ParsedRecord, RecordHead, race_key};

pub fn parse_record(payload: &[u8]) -> Result<ParsedRecord, ParseError> {
    let spec = text(payload, 0, 2)?;
    match spec.as_str() {
        "RA" => Ok(ParsedRecord::Ra(parse_ra(payload)?)),
        "SE" => Ok(ParsedRecord::Se(parse_se(payload)?)),
        "HR" => Ok(ParsedRecord::Hr(parse_hr(payload)?)),
        "UM" => Ok(ParsedRecord::Um(parse_um(payload)?)),
        "KS" => Ok(ParsedRecord::Ks(parse_ks(payload)?)),
        "CH" => Ok(ParsedRecord::Ch(parse_ch(payload)?)),
        "YS" => Ok(ParsedRecord::Ys(parse_ys(payload)?)),
        other => Err(ParseError::UnknownSpec(other.to_string())),
    }
}

/// 1 byte のコードフィールド。空は None。
fn cd(d: &[u8], off: usize) -> Result<Option<String>, ParseError> {
    text_opt(d, off, 1)
}

// ---- RA レース詳細 (JV_RA_RACE, 1272 bytes) ----

fn parse_ra(d: &[u8]) -> Result<RaRace, ParseError> {
    Ok(RaRace {
        head: RecordHead::parse(d)?,
        race_key: race_key(d)?,
        race_date: date(d, 11)?,
        jyo_cd: text(d, 19, 2)?,
        kaiji: num(d, 21, 2)?,
        nichiji: num(d, 23, 2)?,
        race_num: num(d, 25, 2)?,
        youbi_cd: cd(d, 27)?,
        toku_num: num(d, 28, 4)?,
        hondai: text_opt(d, 32, 60)?,
        fukudai: text_opt(d, 92, 60)?,
        kakko: text_opt(d, 152, 60)?,
        ryakusyo10: text_opt(d, 572, 20)?,
        ryakusyo6: text_opt(d, 592, 12)?,
        ryakusyo3: text_opt(d, 604, 6)?,
        grade_cd: cd(d, 614)?,
        syubetu_cd: text_opt(d, 616, 2)?,
        kigo_cd: text_opt(d, 618, 3)?,
        jyuryo_cd: cd(d, 621)?,
        jyoken_name: text_opt(d, 637, 60)?,
        kyori: num(d, 697, 4)?,
        track_cd: text_opt(d, 705, 2)?,
        course_kubun_cd: text_opt(d, 709, 2)?,
        hasso_time: text_opt(d, 873, 4)?,
        toroku_tosu: num(d, 881, 2)?,
        syusso_tosu: num(d, 883, 2)?,
        nyusen_tosu: num(d, 885, 2)?,
        tenko_cd: cd(d, 887)?,
        siba_baba_cd: cd(d, 888)?,
        dirt_baba_cd: cd(d, 889)?,
    })
}

// ---- SE 馬毎レース情報 (JV_SE_RACE_UMA, 555 bytes) ----

fn parse_se(d: &[u8]) -> Result<SeRaceUma, ParseError> {
    Ok(SeRaceUma {
        head: RecordHead::parse(d)?,
        race_key: race_key(d)?,
        wakuban: num(d, 27, 1)?,
        umaban: text(d, 28, 2)?,
        ketto_num: text(d, 30, 10)?,
        bamei: text_opt(d, 40, 36)?,
        uma_kigo_cd: text_opt(d, 76, 2)?,
        sex_cd: cd(d, 78)?,
        hinsyu_cd: cd(d, 79)?,
        keiro_cd: text_opt(d, 80, 2)?,
        barei: num(d, 82, 2)?,
        tozai_cd: cd(d, 84)?,
        chokyosi_code: text_opt(d, 85, 5)?,
        chokyosi_ryakusyo: text_opt(d, 90, 8)?,
        banusi_code: text_opt(d, 98, 6)?,
        banusi_name: text_opt(d, 104, 64)?,
        futan: scaled10(d, 288, 3)?,
        blinker: cd(d, 294)?,
        kisyu_code: text_opt(d, 296, 5)?,
        kisyu_ryakusyo: text_opt(d, 306, 8)?,
        minarai_cd: cd(d, 322)?,
        ba_taijyu: num(d, 324, 3)?,
        zogen_fugo: cd(d, 327)?,
        zogen_sa: num(d, 328, 3)?,
        ijyo_cd: cd(d, 331)?,
        nyusen_jyuni: num(d, 332, 2)?,
        kakutei_jyuni: num(d, 334, 2)?,
        dochaku_kubun: cd(d, 336)?,
        time_sec: packed_time(d, 338)?,
        chakusa_cd: text_opt(d, 342, 3)?,
        jyuni_1c: num(d, 351, 2)?,
        jyuni_2c: num(d, 353, 2)?,
        jyuni_3c: num(d, 355, 2)?,
        jyuni_4c: num(d, 357, 2)?,
        odds: scaled10(d, 359, 4)?,
        ninki: num(d, 363, 2)?,
        honsyokin: num(d, 365, 8)?,
        fukasyokin: num(d, 373, 8)?,
        haron_l3_sec: scaled10(d, 390, 3)?,
    })
}

// ---- HR 払戻 (JV_HR_PAY, 719 bytes) ----

// _PAY_INFO1: Umaban[2] + Pay[9] + Ninki[2]
type PayInfo1 = (String, Option<i64>, Option<i64>);

fn pay_info1(d: &[u8], off: usize) -> Result<PayInfo1, ParseError> {
    Ok((text(d, off, 2)?, num(d, off + 2, 9)?, num(d, off + 11, 2)?))
}

fn parse_hr(d: &[u8]) -> Result<HrPay, ParseError> {
    let head = RecordHead::parse(d)?;
    let key = race_key(d)?;
    let mut payouts = Vec::new();

    // _PAY_INFO1 (Umaban2 + Pay9 + Ninki2): 単勝[3]@102, 複勝[5]@141, 枠連[3]@206
    for (bet_type, base, count) in [
        ("tansho", 102usize, 3usize),
        ("fukusyo", 141, 5),
        ("wakuren", 206, 3),
    ] {
        for i in 0..count {
            let off = base + i * 13;
            let (combo, pay, ninki) = pay_info1(d, off)?;
            if let Some(pay) = pay {
                payouts.push(Payout {
                    bet_type,
                    combo,
                    pay,
                    ninki,
                });
            }
        }
    }
    // _PAY_INFO2 (Kumi4 + Pay9 + Ninki3): 馬連[3]@245, ワイド[7]@293, 馬単[6]@453
    for (bet_type, base, count) in [
        ("umaren", 245usize, 3usize),
        ("wide", 293, 7),
        ("umatan", 453, 6),
    ] {
        for i in 0..count {
            let off = base + i * 16;
            if let Some(pay) = num(d, off + 4, 9)? {
                payouts.push(Payout {
                    bet_type,
                    combo: text(d, off, 4)?,
                    pay,
                    ninki: num(d, off + 13, 3)?,
                });
            }
        }
    }
    // _PAY_INFO3 (Kumi6 + Pay9 + Ninki3): 3連複[3]@549
    for i in 0..3usize {
        let off = 549 + i * 18;
        if let Some(pay) = num(d, off + 6, 9)? {
            payouts.push(Payout {
                bet_type: "sanrenpuku",
                combo: text(d, off, 6)?,
                pay,
                ninki: num(d, off + 15, 3)?,
            });
        }
    }
    // _PAY_INFO4 (Kumi6 + Pay9 + Ninki4): 3連単[6]@603
    for i in 0..6usize {
        let off = 603 + i * 19;
        if let Some(pay) = num(d, off + 6, 9)? {
            payouts.push(Payout {
                bet_type: "sanrentan",
                combo: text(d, off, 6)?,
                pay,
                ninki: num(d, off + 15, 4)?,
            });
        }
    }
    Ok(HrPay {
        head,
        race_key: key,
        payouts,
    })
}

// ---- UM 競走馬マスタ (JV_UM_UMA, 1609 bytes) ----

fn parse_um(d: &[u8]) -> Result<UmUma, ParseError> {
    // Ketto3Info[14]: HansyokuNum[10] + Bamei[36]。0=父, 1=母。
    let sire = text_opt(d, 204 + 10, 36)?;
    let dam = text_opt(d, 204 + 46 + 10, 36)?;
    Ok(UmUma {
        head: RecordHead::parse(d)?,
        ketto_num: text(d, 11, 10)?,
        del_kubun: cd(d, 21)?,
        reg_date: date(d, 22)?,
        del_date: date(d, 30)?,
        birth_date: date(d, 38)?,
        bamei: text_opt(d, 46, 36)?,
        bamei_kana: text_opt(d, 82, 36)?,
        bamei_eng: text_opt(d, 118, 60)?,
        sex_cd: cd(d, 200)?,
        hinsyu_cd: cd(d, 201)?,
        keiro_cd: text_opt(d, 202, 2)?,
        sire_name: sire,
        dam_name: dam,
        tozai_cd: cd(d, 848)?,
        chokyosi_code: text_opt(d, 849, 5)?,
        chokyosi_ryakusyo: text_opt(d, 854, 8)?,
        breeder_code: text_opt(d, 882, 8)?,
        breeder_name: text_opt(d, 890, 72)?,
        sanchi_name: text_opt(d, 962, 20)?,
        banusi_code: text_opt(d, 982, 6)?,
        banusi_name: text_opt(d, 988, 64)?,
        race_count: num(d, 1604, 3)?,
    })
}

// ---- KS 騎手マスタ (JV_KS_KISYU, 4173 bytes) ----

fn parse_ks(d: &[u8]) -> Result<KsKisyu, ParseError> {
    Ok(KsKisyu {
        head: RecordHead::parse(d)?,
        kisyu_code: text(d, 11, 5)?,
        del_kubun: cd(d, 16)?,
        kisyu_name: text_opt(d, 41, 34)?,
        kisyu_name_kana: text_opt(d, 109, 30)?,
        kisyu_ryakusyo: text_opt(d, 139, 8)?,
        kisyu_name_eng: text_opt(d, 147, 80)?,
        sex_cd: cd(d, 227)?,
        minarai_cd: cd(d, 229)?,
        tozai_cd: cd(d, 230)?,
        chokyosi_code: text_opt(d, 251, 5)?,
    })
}

// ---- CH 調教師マスタ (JV_CH_CHOKYOSI, 3862 bytes) ----

fn parse_ch(d: &[u8]) -> Result<ChChokyosi, ParseError> {
    Ok(ChChokyosi {
        head: RecordHead::parse(d)?,
        chokyosi_code: text(d, 11, 5)?,
        del_kubun: cd(d, 16)?,
        chokyosi_name: text_opt(d, 41, 34)?,
        chokyosi_name_kana: text_opt(d, 75, 30)?,
        chokyosi_ryakusyo: text_opt(d, 105, 8)?,
        sex_cd: cd(d, 193)?,
        tozai_cd: cd(d, 194)?,
    })
}

// ---- YS 開催スケジュール (JV_YS_SCHEDULE, 382 bytes) ----

fn parse_ys(d: &[u8]) -> Result<YsSchedule, ParseError> {
    let mut jyusyo = Vec::new();
    for i in 0..3usize {
        // _JYUSYO_INFO: TokuNum4 + Hondai60 + Ryakusyo10 20 + Ryakusyo6 12
        //             + Ryakusyo3 6 + Nkai3 + GradeCD1 + SyubetuCD2
        //             + KigoCD3 + JyuryoCD1 + Kyori4 + TrackCD2 = 118
        let off = 26 + i * 118;
        let hondai = text_opt(d, off + 4, 60)?;
        if hondai.is_none() {
            continue;
        }
        jyusyo.push(YsJyusyo {
            toku_num: num(d, off, 4)?,
            hondai,
            ryakusyo10: text_opt(d, off + 64, 20)?,
            grade_cd: cd(d, off + 105)?,
            kyori: num(d, off + 112, 4)?,
            track_cd: text_opt(d, off + 116, 2)?,
        });
    }
    Ok(YsSchedule {
        head: RecordHead::parse(d)?,
        race_date: date(d, 11)?,
        jyo_cd: text(d, 19, 2)?,
        kaiji: num(d, 21, 2)?,
        nichiji: num(d, 23, 2)?,
        youbi_cd: cd(d, 25)?,
        jyusyo,
    })
}

/// 生レコードの最小 sanity check (長さとヘッダ)。parse_record の前に使う。
pub fn record_type(payload: &[u8]) -> Result<&str, ParseError> {
    let spec = raw(payload, 0, 2)?;
    core::str::from_utf8(spec).map_err(|_| ParseError::UnknownSpec("??".into()))
}
