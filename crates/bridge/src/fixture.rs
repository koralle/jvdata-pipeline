//! 動作確認用の合成 JV-Data fixture を生成する。
//!
//! 生成するバイト列は JVData_Structure.h のレイアウトどおり (固定長 + CRLF 終端)。
//! 実データではなく「レイアウトが正しい合成データ」なので、parser → normalize →
//! SQL join の縦筋を JV-Link 無しで検証できる。

use encoding_rs::SHIFT_JIS;
use std::io;
use std::path::Path;

fn put(d: &mut [u8], off: usize, ascii: &str) {
    let (b, _, _) = SHIFT_JIS.encode(ascii);
    d[off..off + b.len()].copy_from_slice(&b);
}

/// 0 埋めの右寄せ数値フィールド。
fn putn(d: &mut [u8], off: usize, len: usize, v: i64) {
    let s = format!("{v:0>width$}", width = len);
    put(d, off, &s);
}

/// レコードバッファ: spec + kubun + makedate + 本体(空白) + CRLF
fn rec(spec: &str, kubun: &str, make: &str, total: usize) -> Vec<u8> {
    let mut d = vec![b' '; total];
    put(&mut d, 0, spec);
    put(&mut d, 2, kubun);
    put(&mut d, 3, make);
    d[total - 2] = b'\r';
    d[total - 1] = b'\n';
    d
}

fn head_race(d: &mut [u8], ymd: &str, jyo: &str, kaiji: &str, nichiji: &str, race: &str) {
    // _RACE_ID: Year4 MonthDay4 JyoCD2 Kaiji2 Nichiji2 RaceNum2 @11
    put(d, 11, &ymd[0..4]);
    put(d, 15, &ymd[4..8]);
    put(d, 19, jyo);
    put(d, 21, kaiji);
    put(d, 23, nichiji);
    put(d, 25, race);
}

/// 合成 RA (1272 bytes)
pub fn ra(ymd: &str, jyo: &str, kaiji: &str, nichiji: &str, race: &str, name: &str) -> Vec<u8> {
    let mut d = rec("RA", "7", "20240513", 1272);
    head_race(&mut d, ymd, jyo, kaiji, nichiji, race);
    put(&mut d, 27, "7"); // YoubiCD (日)
    put(&mut d, 32, name); // Hondai (SJIS)
    put(&mut d, 614, "A"); // GradeCD
    put(&mut d, 616, "04"); // SyubetuCD
    putn(&mut d, 697, 4, 1600); // Kyori
    put(&mut d, 705, "11"); // TrackCD (芝左)
    put(&mut d, 873, "1525"); // HassoTime
    putn(&mut d, 881, 2, 4); // TorokuTosu
    putn(&mut d, 883, 2, 4); // SyussoTosu
    putn(&mut d, 885, 2, 4); // NyusenTosu
    put(&mut d, 887, "1"); // TenkoCD 晴
    put(&mut d, 888, "1"); // SibaBabaCD 良
    d
}

/// 合成 SE (555 bytes)
#[allow(clippy::too_many_arguments)]
pub fn se(
    ymd: &str,
    jyo: &str,
    kaiji: &str,
    nichiji: &str,
    race: &str,
    umaban: i64,
    ketto: &str,
    bamei: &str,
    kisyu: &str,
    jyuni: i64,
    time_msss: i64,
    odds: i64,
    ninki: i64,
) -> Vec<u8> {
    let mut d = rec("SE", "7", "20240513", 555);
    head_race(&mut d, ymd, jyo, kaiji, nichiji, race);
    putn(&mut d, 27, 1, (umaban + 1) / 2); // Wakuban (仮)
    putn(&mut d, 28, 2, umaban); // Umaban
    put(&mut d, 30, ketto); // KettoNum
    put(&mut d, 40, bamei); // Bamei
    putn(&mut d, 82, 2, 4); // Barei
    put(&mut d, 84, "1"); // TozaiCD
    put(&mut d, 85, "01234"); // ChokyosiCode
    put(&mut d, 90, "美浦テスト"); // ChokyosiRyakusyo
    putn(&mut d, 288, 3, 570); // Futan 57.0kg
    put(&mut d, 296, kisyu); // KisyuCode
    put(&mut d, 306, "テスト騎手"); // KisyuRyakusyo
    putn(&mut d, 324, 3, 480); // BaTaijyu
    put(&mut d, 327, "+"); // ZogenFugo
    putn(&mut d, 328, 3, 4); // ZogenSa
    put(&mut d, 331, "0"); // IJyoCD
    putn(&mut d, 332, 2, jyuni); // NyusenJyuni
    putn(&mut d, 334, 2, jyuni); // KakuteiJyuni
    putn(&mut d, 338, 4, time_msss); // Time ("9分99秒9" packed。例: 1335 = 1'33"5)
    putn(&mut d, 359, 4, odds); // Odds ×10
    putn(&mut d, 363, 2, ninki); // Ninki
    putn(&mut d, 365, 8, if jyuni == 1 { 8000000 } else { 0 }); // Honsyokin
    d
}

/// 合成 HR (719 bytes)。tansho/fukusyo のみ埋める。
pub fn hr(
    ymd: &str,
    jyo: &str,
    kaiji: &str,
    nichiji: &str,
    race: &str,
    tansho: (i64, i64),
) -> Vec<u8> {
    let mut d = rec("HR", "7", "20240513", 719);
    head_race(&mut d, ymd, jyo, kaiji, nichiji, race);
    putn(&mut d, 27, 2, 4); // TorokuTosu
    putn(&mut d, 29, 2, 4); // SyussoTosu
    // PayTansyo[0] @102: Umaban2 + Pay9 + Ninki2
    putn(&mut d, 102, 2, tansho.0);
    putn(&mut d, 104, 9, tansho.1);
    putn(&mut d, 113, 2, 1);
    d
}

/// 合成 UM (1609 bytes)
pub fn um(ketto: &str, bamei: &str, sire: &str, dam: &str, birth: &str) -> Vec<u8> {
    let mut d = rec("UM", "1", "20240508", 1609); // データ区分 1:新規馬名登録
    put(&mut d, 11, ketto);
    put(&mut d, 22, "20220601"); // RegDate
    put(&mut d, 38, birth); // BirthDate
    put(&mut d, 46, bamei); // Bamei
    put(&mut d, 204 + 10, sire); // Ketto3Info[0].Bamei = 父
    put(&mut d, 204 + 46 + 10, dam); // Ketto3Info[1].Bamei = 母
    put(&mut d, 848, "1"); // TozaiCD
    put(&mut d, 849, "01234"); // ChokyosiCode
    put(&mut d, 854, "美浦テスト");
    putn(&mut d, 1604, 3, 12); // RaceCount
    d
}

/// 合成 KS (4173 bytes)
pub fn ks(code: &str, name: &str) -> Vec<u8> {
    let mut d = rec("KS", "1", "20240508", 4173); // データ区分 1:新規登録
    put(&mut d, 11, code);
    put(&mut d, 33, "19900101"); // BirthDate
    put(&mut d, 41, name); // KisyuName
    put(&mut d, 139, "テスト"); // KisyuRyakusyo
    put(&mut d, 230, "1"); // TozaiCD
    put(&mut d, 251, "01234"); // ChokyosiCode
    d
}

/// 合成 CH (3862 bytes)
pub fn ch(code: &str, name: &str) -> Vec<u8> {
    let mut d = rec("CH", "1", "20240508", 3862); // データ区分 1:新規登録
    put(&mut d, 11, code);
    put(&mut d, 33, "19700101");
    put(&mut d, 41, name);
    put(&mut d, 105, "テスト厩舎");
    put(&mut d, 194, "1"); // TozaiCD
    d
}

/// 合成 YS (382 bytes)
pub fn ys(ymd: &str, jyo: &str, kaiji: &str, nichiji: &str, g1name: &str) -> Vec<u8> {
    let mut d = rec("YS", "3", "20240508", 382); // データ区分 3:開催終了(成績確定時点)
    put(&mut d, 11, &ymd[0..4]);
    put(&mut d, 15, &ymd[4..8]);
    put(&mut d, 19, jyo);
    put(&mut d, 21, kaiji);
    put(&mut d, 23, nichiji);
    put(&mut d, 25, "7"); // YoubiCD
    // JyusyoInfo[0] @26: TokuNum4 + Hondai60 + ...
    putn(&mut d, 26, 4, 5);
    put(&mut d, 30, g1name);
    put(&mut d, 26 + 105, "A"); // GradeCD
    putn(&mut d, 26 + 112, 4, 1600);
    put(&mut d, 26 + 116, "11");
    d
}

/// demo 用データセットを dir に書き出す。manifest.txt つき。
///
/// 内容: 2024-05-12 東京 (JyoCD=05) 第5回8日目 11R/12R、各 4 頭立て。
pub fn write_demo_dataset(dir: &Path) -> io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    let (y, j, k, n) = ("20240512", "05", "05", "08");

    let mut manifest = Vec::new();
    let mut write = |name: &str, recs: Vec<Vec<u8>>| -> io::Result<()> {
        let mut f = Vec::new();
        for r in recs {
            f.extend_from_slice(&r);
        }
        std::fs::write(dir.join(name), f)?;
        manifest.push(name.to_string());
        Ok(())
    };

    write(
        "YSSW20240508000000.jvd",
        vec![ys(y, j, k, n, "ＮＨＫマイルカップ")],
    )?;
    write(
        "UMSW20240508000000.jvd",
        vec![
            um(
                "2020110001",
                "テストイチ",
                "キズナ",
                "テストハハ",
                "20200301",
            ),
            um(
                "2020110002",
                "テストニ",
                "エピファネイア",
                "テストハハ２",
                "20200402",
            ),
            um(
                "2020110003",
                "テストサン",
                "ドゥラメンテ",
                "テストハハ３",
                "20200315",
            ),
            um(
                "2020110004",
                "テストヨン",
                "ハーツクライ",
                "テストハハ４",
                "20200420",
            ),
        ],
    )?;
    write(
        "KSSW20240508000000.jvd",
        vec![ks("00866", "テスト騎手一郎"), ks("01166", "テスト騎手二郎")],
    )?;
    write("CHSW20240508000000.jvd", vec![ch("01234", "テスト調教師")])?;
    write(
        "RASW20240512000000.jvd",
        vec![
            ra(y, j, k, n, "11", "テストステークス"),
            se(
                y,
                j,
                k,
                n,
                "11",
                1,
                "2020110001",
                "テストイチ",
                "00866",
                1,
                1335,
                25,
                1,
            ),
            se(
                y,
                j,
                k,
                n,
                "11",
                2,
                "2020110002",
                "テストニ",
                "01166",
                2,
                1338,
                67,
                3,
            ),
            se(
                y,
                j,
                k,
                n,
                "11",
                3,
                "2020110003",
                "テストサン",
                "00866",
                3,
                1340,
                121,
                4,
            ),
            se(
                y,
                j,
                k,
                n,
                "11",
                4,
                "2020110004",
                "テストヨン",
                "01166",
                4,
                1352,
                45,
                2,
            ),
            hr(y, j, k, n, "11", (1, 250)),
            ra(y, j, k, n, "12", "テスト特別"),
            se(
                y,
                j,
                k,
                n,
                "12",
                1,
                "2020110002",
                "テストニ",
                "01166",
                2,
                1380,
                55,
                2,
            ),
            se(
                y,
                j,
                k,
                n,
                "12",
                2,
                "2020110003",
                "テストサン",
                "00866",
                1,
                1378,
                30,
                1,
            ),
        ],
    )?;

    std::fs::write(dir.join("manifest.txt"), manifest.join("\n") + "\n")?;
    Ok(manifest.len())
}
