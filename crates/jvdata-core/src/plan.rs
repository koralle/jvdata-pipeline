//! バックフィルの window 分割と再開ロジック (pure functions)。
//!
//! JV-Link 仕様 (インターフェース仕様書 4.2.2) に基づく:
//!   - option=4 (setup) の fromtime は「コンテンツの日付」(= 開催日) で解釈される
//!   - fromtime は `YYYYMMDDhhmmss` または `YYYYMMDDhhmmss-YYYYMMDDhhmmss`
//!   - TOKU/DIFF/DIFN/HOSE/HOSN/HOYU/COMM は終了時刻を指定できず
//!     「開始以降の全データ」しか取れない
//!   - セットアップの最終回は終了時刻なしで実行する (以降は option=1 差分へ)

use crate::dataspec::Dataspec;
use time::Date;

/// JVOpen の読み出しポイント時刻文字列 (YYYYMMDDDhhmmss)。
fn ts(d: Date, hms: &str) -> String {
    format!(
        "{}{:02}{:02}{}",
        d.year(),
        u8::from(d.month()),
        d.day(),
        hms
    )
}

/// 1 window = 1 回の JVOpen + ファイル列の読み出し。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub dataspec: Dataspec,
    /// `YYYYMMDDhhmmss`
    pub start: String,
    /// `Some` → `start-end` で JVOpen。`None` → open-ended。
    pub end: Option<String>,
}

impl Window {
    /// JVOpen の fromtime 引数。
    pub fn fromtime(&self) -> String {
        match &self.end {
            Some(end) => format!("{}-{}", self.start, end),
            None => self.start.clone(),
        }
    }

    /// checkpoints の PK 部品。
    pub fn key(&self) -> (String, String) {
        (self.dataspec.id().to_string(), self.start.clone())
    }
}

/// `from`〜`to` (両端含む、コンテンツ日付) の setup window 列を作る。
///
/// - 終了時刻を指定できる dataspec は暦年で分割 (1 window ≒ 1 年分)
/// - 指定できない dataspec (DIFF 等) は open-ended 1 window
/// - window の区切りは SDK 開発ガイドの手順どおり「次の FROM =
///   前回の END と同じ時刻」。fromtime は「より大きい」(排他) で
///   終了時刻は「まで」(包含) なので、同じ時刻を共有する区切り方が
///   唯一取りこぼしも重複もない切り方になる
/// - `to >= today` のとき最後の window は open-ended にする
///   (仕様書: セットアップの最終回は終了時刻を指定しない)。
///   open-ended window は以降の全データを含むので、その先に
///   window は作らない
pub fn setup_windows(spec: Dataspec, from: Date, to: Date, today: Date) -> Vec<Window> {
    if !spec.supports_end_time() {
        return vec![Window {
            dataspec: spec,
            start: ts(from, "000000"),
            end: None,
        }];
    }

    let mut windows = Vec::new();
    let mut year = from.year();
    let mut prev_end: Option<Date> = None;
    while year <= to.year() {
        let jan1 = Date::from_calendar_date(year, time::Month::January, 1);
        let dec31 = Date::from_calendar_date(year, time::Month::December, 31);
        let (Ok(jan1), Ok(dec31)) = (jan1, dec31) else {
            break;
        };
        let start = match prev_end {
            // 2 window 目以降の FROM は前 window の END と同じ時刻。
            // (YYYY0101000000 開始にすると丁度その 1 秒の timestamp を
            //   持つファイルがどの window にも入らない)
            Some(end) => ts(end, "235959"),
            None => ts(std::cmp::max(from, jan1), "000000"),
        };
        let end = std::cmp::min(to, dec31);
        // to が今日以降なら最終 window は open-ended (仕様書の「最後は終了時刻なし」)
        let end_ts = if end >= today {
            None
        } else {
            Some(ts(end, "235959"))
        };
        let open_ended = end_ts.is_none();
        windows.push(Window {
            dataspec: spec,
            start,
            end: end_ts,
        });
        // open-ended window は以降の全データを取る。その先の window は
        // 完全に内側になるので作っても無駄 (冪等だが二重取得になる)
        if open_ended {
            break;
        }
        prev_end = Some(end);
        year += 1;
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn splits_by_year() {
        let w = setup_windows(
            Dataspec::Race,
            date!(2016 - 01 - 01),
            date!(2018 - 06 - 30),
            date!(2026 - 01 - 01),
        );
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].start, "20160101000000");
        assert_eq!(w[0].end.as_deref(), Some("20161231235959"));
        // 次の FROM は前 window の END と同じ時刻 (SDK ガイドの区切り手順)。
        // fromtime は排他・end は包含なので境界が 1 秒も抜けない。
        assert_eq!(w[1].start, "20161231235959");
        assert_eq!(w[1].end.as_deref(), Some("20171231235959"));
        assert_eq!(w[2].start, "20171231235959");
        assert_eq!(w[2].end.as_deref(), Some("20180630235959"));
    }

    #[test]
    fn nothing_after_open_ended_window() {
        // to が今日より先 → その年の window は open-ended。
        // open-ended は以降の全データを含むので、後続の年の window は作らない。
        let w = setup_windows(
            Dataspec::Race,
            date!(2025 - 01 - 01),
            date!(2027 - 06 - 30),
            date!(2026 - 01 - 01),
        );
        assert_eq!(w.len(), 2);
        assert_eq!(w[1].start, "20251231235959");
        assert_eq!(w[1].end, None);
    }

    #[test]
    fn open_ended_when_to_is_today_or_later() {
        let w = setup_windows(
            Dataspec::Race,
            date!(2016 - 01 - 01),
            date!(2026 - 09 - 21),
            date!(2026 - 09 - 21),
        );
        assert_eq!(w.last().map(|w| w.end.as_deref()), Some(None));
    }

    #[test]
    fn diff_is_single_open_ended() {
        let w = setup_windows(
            Dataspec::Diff,
            date!(2016 - 01 - 01),
            date!(2020 - 12 - 31),
            date!(2026 - 01 - 01),
        );
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].end, None);
        assert_eq!(w[0].fromtime(), "20160101000000");
    }

    #[test]
    fn fromtime_range() {
        let w = Window {
            dataspec: Dataspec::Race,
            start: "20160101000000".into(),
            end: Some("20161231235959".into()),
        };
        assert_eq!(w.fromtime(), "20160101000000-20161231235959");
    }
}
