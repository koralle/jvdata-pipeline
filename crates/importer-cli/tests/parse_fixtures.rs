//! fixture (spec レイアウトの合成 bytes) → parser の往復テスト。
//!
//! `bridge::fixture` が JVData_Structure.h のレイアウトで生成したバイト列を
//! `jvdata_core::parse_record` が正しく decode できることを検証する。
//! Shift_JIS の日本語フィールドと数値フィールドの両方を見る。

use jvdata_core::parse::parse_record;
use jvdata_core::record::ParsedRecord;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn ra_roundtrip() -> TestResult {
    let d = bridge::fixture::ra("20240512", "05", "05", "08", "11", "テストステークス");
    let ParsedRecord::Ra(r) = parse_record(&d)? else {
        return Err("expected RA variant".into());
    };
    assert_eq!(r.head.record_type, "RA");
    assert_eq!(r.head.data_kubun, "7");
    assert_eq!(r.race_key, "2024051205050811");
    assert_eq!(r.jyo_cd, "05");
    assert_eq!(r.race_num, Some(11));
    assert_eq!(r.hondai.as_deref(), Some("テストステークス"));
    assert_eq!(r.kyori, Some(1600));
    assert_eq!(r.hasso_time.as_deref(), Some("1525"));
    assert_eq!(r.toroku_tosu, Some(4));
    Ok(())
}

#[test]
fn se_roundtrip() -> TestResult {
    let d = bridge::fixture::se(
        "20240512",
        "05",
        "05",
        "08",
        "11",
        3,
        "2020110003",
        "テストサン",
        "00866",
        1,
        1335,
        121,
        4,
    );
    let ParsedRecord::Se(r) = parse_record(&d)? else {
        return Err("expected SE variant".into());
    };
    assert_eq!(r.race_key, "2024051205050811");
    assert_eq!(r.umaban, "03");
    assert_eq!(r.ketto_num, "2020110003");
    assert_eq!(r.bamei.as_deref(), Some("テストサン"));
    assert_eq!(r.barei, Some(4));
    assert_eq!(r.futan, Some(57.0)); // "570" = 57.0kg
    assert_eq!(r.kisyu_code.as_deref(), Some("00866"));
    assert_eq!(r.ba_taijyu, Some(480));
    assert_eq!(r.zogen_fugo.as_deref(), Some("+"));
    assert_eq!(r.kakutei_jyuni, Some(1));
    assert_eq!(r.time_sec, Some(93.5)); // "1335" = 1'33"5 = 93.5s
    assert_eq!(r.odds, Some(12.1)); // "0121" = 12.1倍
    assert_eq!(r.ninki, Some(4));
    assert_eq!(r.honsyokin, Some(8000000));
    Ok(())
}

#[test]
fn hr_tansho_roundtrip() -> TestResult {
    let d = bridge::fixture::hr("20240512", "05", "05", "08", "11", (1, 250));
    let ParsedRecord::Hr(r) = parse_record(&d)? else {
        return Err("expected HR variant".into());
    };
    let tansho: Vec<_> = r
        .payouts
        .iter()
        .filter(|p| p.bet_type == "tansho")
        .collect();
    assert_eq!(tansho.len(), 1);
    assert_eq!(tansho[0].combo, "01");
    assert_eq!(tansho[0].pay, 250);
    assert_eq!(tansho[0].ninki, Some(1));
    Ok(())
}

#[test]
fn um_roundtrip() -> TestResult {
    let d = bridge::fixture::um(
        "2020110001",
        "テストイチ",
        "キズナ",
        "テストハハ",
        "20200301",
    );
    let ParsedRecord::Um(r) = parse_record(&d)? else {
        return Err("expected UM variant".into());
    };
    assert_eq!(r.ketto_num, "2020110001");
    assert_eq!(r.bamei.as_deref(), Some("テストイチ"));
    assert_eq!(r.sire_name.as_deref(), Some("キズナ"));
    assert_eq!(r.dam_name.as_deref(), Some("テストハハ"));
    assert_eq!(r.chokyosi_code.as_deref(), Some("01234"));
    assert_eq!(r.race_count, Some(12));
    assert_eq!(
        r.birth_date.ok_or("birth_date missing")?.to_string(),
        "2020-03-01"
    );
    Ok(())
}

#[test]
fn ks_ch_ys_roundtrip() -> TestResult {
    let d = bridge::fixture::ks("00866", "テスト騎手一郎");
    let ParsedRecord::Ks(r) = parse_record(&d)? else {
        return Err("expected KS variant".into());
    };
    assert_eq!(r.kisyu_code, "00866");
    assert_eq!(r.kisyu_name.as_deref(), Some("テスト騎手一郎"));

    let d = bridge::fixture::ch("01234", "テスト調教師");
    let ParsedRecord::Ch(r) = parse_record(&d)? else {
        return Err("expected CH variant".into());
    };
    assert_eq!(r.chokyosi_code, "01234");
    assert_eq!(r.chokyosi_name.as_deref(), Some("テスト調教師"));

    let d = bridge::fixture::ys("20240512", "05", "05", "08", "ＮＨＫマイルカップ");
    let ParsedRecord::Ys(r) = parse_record(&d)? else {
        return Err("expected YS variant".into());
    };
    assert_eq!(r.jyo_cd, "05");
    assert_eq!(r.kaiji, Some(5));
    assert_eq!(r.jyusyo.len(), 1);
    assert_eq!(r.jyusyo[0].hondai.as_deref(), Some("ＮＨＫマイルカップ"));
    assert_eq!(r.jyusyo[0].kyori, Some(1600));
    Ok(())
}

#[test]
fn unknown_spec_is_error_not_panic() {
    let mut d = vec![b' '; 100];
    d[0] = b'Z';
    d[1] = b'Z';
    assert!(parse_record(&d).is_err());
}
