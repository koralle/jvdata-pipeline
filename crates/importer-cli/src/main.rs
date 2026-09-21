//! jvdata — JRA-VAN → JV-Link → PostgreSQL ingestion CLI
//!
//!   jvdata migrate
//!   jvdata backfill --from 2016-01-01 --to 2026-09-21 [--dataspec RACE,DIFF,YSCH]
//!   jvdata resume      # pending window の続き + 差分取得
//!   jvdata status
//!   jvdata normalize   # raw_records → jv.* へ反映
//!   jvdata reparse     # parse 失敗/未対応の raw を再処理 (parser 修正後)
//!   jvdata abandon --dataspec RACE --window-start 20250101000000
//!                      # 恒久的に失敗する window を実行対象から隔離
//!   jvdata gen-fixture DIR   # JV-Link 無しで試せる合成データを生成

mod ingest;

use bridge::source::{Source, SourceError, make_source};
use clap::{Parser, Subcommand};
use ingest::{IngestError, run_diff, run_window};
use jvdata_core::dataspec::Dataspec;
use jvdata_core::plan::{Window, setup_windows};
use miette::{Context, IntoDiagnostic, Result};
use std::path::PathBuf;
use store::Store;
use time::Date;
use time::macros::format_description;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const DATE_FMT: &[time::format_description::FormatItem<'static>] =
    format_description!("[year]-[month]-[day]");

#[derive(Parser)]
#[command(name = "jvdata", about = "JRA-VAN → PostgreSQL ingestion")]
struct Cli {
    /// PostgreSQL URL。省略時は DATABASE_URL。
    #[arg(long, env = "DATABASE_URL", global = true)]
    database_url: Option<String>,

    /// raw の取得元。"wine" (既定) | "fixture:DIR" | 任意の bridge コマンド。
    #[arg(long, global = true, default_value = "wine", env = "JVDATA_SOURCE")]
    source: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// ingest/jv スキーマの migration を適用する
    Migrate,
    /// 蓄積系セットアップ取得 (option=4) を window 分割で実行する。
    /// 既存 checkpoint と同じ window は冪等 (done は skip)。
    Backfill {
        /// 開始日 (コンテンツ日付 = 開催日)。例: 2016-01-01
        #[arg(long)]
        from: String,
        /// 終了日。例: 2026-09-21
        #[arg(long)]
        to: String,
        /// 対象 dataspec (カンマ区切り)。省略時は RACE,DIFF,YSCH
        #[arg(long, value_delimiter = ',')]
        dataspec: Option<Vec<String>>,
    },
    /// 中断した window を再開し、残りと差分取得を実行する
    Resume,
    /// ingest/jv の状態を表示する
    Status,
    /// raw_records の pending 分を jv.* へ反映する
    Normalize,
    /// 恒久的に失敗し続ける window の checkpoint を 'abandoned' にし、
    /// resume/backfill の実行対象から外す。
    /// 復活させたい場合は window_end が変わるよう --to を変えて
    /// backfill を seed し直す (範囲変更で pending に戻る)。
    Abandon {
        /// dataspec ID (例: RACE)
        #[arg(long)]
        dataspec: String,
        /// 対象 window の window_start (YYYYMMDDhhmmss)
        #[arg(long)]
        window_start: String,
    },
    /// parse 失敗・未対応の raw_records を parse_state=0 に戻して再処理する
    /// (parser を修正したあとに使う)。--all で全 raw を再 parse する。
    Reparse {
        /// normalized 済みを含む全レコードを再 parse する
        #[arg(long)]
        all: bool,
    },
    /// 動作確認用の合成 fixture を DIR に生成する
    GenFixture { dir: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();

    if let Cmd::GenFixture { dir } = cli.cmd {
        let n = bridge::fixture::write_demo_dataset(&dir).into_diagnostic()?;
        println!("wrote {n} files to {}", dir.display());
        return Ok(());
    }

    let store = match &cli.database_url {
        Some(url) => Store::connect(url).await.into_diagnostic()?,
        None => Store::connect_options(connect_options_from_env()?)
            .await
            .into_diagnostic()?,
    };

    // ingest 系コマンドは同時に 1 プロセスに限る。2 つ目が走ると
    // 進行中の run が 'interrupted' に畳まれ、interleave した commit で
    // checkpoint の last_file が前後に揺れる (冪等キーで欠損はしないが
    // 再開が遠回しになる)。status は読むだけなのでロックしない。
    let _lock = match cli.cmd {
        Cmd::Status => None,
        _ => Some(acquire_ingest_lock(&store).await?),
    };

    match cli.cmd {
        Cmd::Migrate => {
            store.migrate().await.into_diagnostic()?;
            println!("migrations applied");
        }
        Cmd::Backfill { from, to, dataspec } => {
            let from = parse_date(&from)?;
            let to = parse_date(&to)?;
            if from > to {
                return Err(miette::miette!("--from ({from}) is after --to ({to})"));
            }
            let specs = parse_dataspecs(dataspec)?;
            let source = make_source(&cli.source);
            backfill(&store, source.as_ref(), &specs, from, to).await?;
        }
        Cmd::Resume => {
            let source = make_source(&cli.source);
            resume(&store, source.as_ref()).await?;
        }
        Cmd::Status => {
            status(&store).await?;
        }
        Cmd::Abandon {
            dataspec,
            window_start,
        } => {
            let spec: Dataspec = dataspec.parse().map_err(|e| miette::miette!("{e}"))?;
            match store
                .abandon_window(spec.id(), &window_start)
                .await
                .into_diagnostic()?
            {
                None => {
                    return Err(miette::miette!(
                        "checkpoint not found: {} {window_start}",
                        spec.id()
                    ));
                }
                Some(prev) if prev == "done" || prev == "abandoned" => {
                    println!("{} {window_start}: already {prev}", spec.id());
                }
                Some(prev) => {
                    println!("{} {window_start}: {prev} -> abandoned", spec.id());
                }
            }
        }
        Cmd::Normalize => {
            let s = store.normalize_pending().await.into_diagnostic()?;
            println!(
                "normalized={} parse_errors={} skipped_unknown={} (processed={})",
                s.normalized, s.parse_errors, s.skipped_unknown, s.processed
            );
        }
        Cmd::Reparse { all } => {
            let n = store.reset_parse_state(all).await.into_diagnostic()?;
            println!("reset {n} raw records to pending");
            let s = store.normalize_pending().await.into_diagnostic()?;
            println!(
                "normalized={} parse_errors={} skipped_unknown={} (processed={})",
                s.normalized, s.parse_errors, s.skipped_unknown, s.processed
            );
        }
        // 上で early-return 済み
        Cmd::GenFixture { .. } => {}
    }
    Ok(())
}

fn parse_date(s: &str) -> Result<Date> {
    Date::parse(s, DATE_FMT).into_diagnostic()
}

fn parse_dataspecs(v: Option<Vec<String>>) -> Result<Vec<Dataspec>> {
    match v {
        None => Ok(Dataspec::DEFAULT_BACKFILL.to_vec()),
        Some(v) => v
            .iter()
            .map(|s| s.parse::<Dataspec>().map_err(|e| miette::miette!("{e}")))
            .collect(),
    }
}

/// POSTGRES_* 環境変数から接続オプションを組み立てる。
/// URL 文字列を経由しないので、パスワードに URL の区切り文字
/// (@, /, #, ?, %) が含まれていても壊れない。
fn connect_options_from_env() -> Result<sqlx::postgres::PgConnectOptions> {
    let Some(pass) = std::env::var("POSTGRES_PASSWORD").ok() else {
        return Err(miette::miette!(
            "DATABASE_URL が未設定です。\n  例: postgresql://jvdata:$POSTGRES_PASSWORD@127.0.0.1:5432/jvdata"
        ));
    };
    let host = std::env::var("POSTGRES_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port = std::env::var("POSTGRES_PORT")
        .unwrap_or_else(|_| "5432".into())
        .parse::<u16>()
        .into_diagnostic()
        .wrap_err("POSTGRES_PORT must be a TCP port number")?;
    let db = std::env::var("POSTGRES_DB").unwrap_or_else(|_| "jvdata".into());
    let user = std::env::var("POSTGRES_USER").unwrap_or_else(|_| "jvdata".into());
    // new_without_pgpass: 明示設定なので ~/.pgpass の資格情報は拾わない
    Ok(sqlx::postgres::PgConnectOptions::new_without_pgpass()
        .host(&host)
        .port(port)
        .username(&user)
        .password(&pass)
        .database(&db))
}

/// ingest 系コマンドの排他ロック。保持中の他プロセスがあれば即エラー。
async fn acquire_ingest_lock(store: &Store) -> Result<store::IngestLock> {
    store
        .try_ingest_lock()
        .await
        .into_diagnostic()?
        .ok_or_else(|| miette::miette!("別の jvdata プロセスが実行中です (advisory lock 取得失敗)"))
}

/// backfill: window を seed して pending を順に実行し、最後に normalize する。
/// window の失敗は normalize を妨げない: commit 済み raw を滞留させない
/// ため、pending の失敗は normalize のあとに伝播する。
async fn backfill(
    store: &Store,
    source: &dyn Source,
    specs: &[Dataspec],
    from: Date,
    to: Date,
) -> Result<()> {
    let today = time::OffsetDateTime::now_utc().date();
    for spec in specs {
        for w in setup_windows(*spec, from, to, today) {
            store
                .seed_window(spec.id(), &w.start, w.end.as_deref(), "setup")
                .await
                .into_diagnostic()?;
        }
    }
    let pending_result = run_pending(store, source).await;
    if let Err(e) = &pending_result {
        warn!(error = %e, "window failed; normalizing committed records anyway");
    }
    // backfill 1 コマンドで jv.* まで分析可能状態にする
    let norm_result = normalize_pending(store).await;
    if let Err(e) = &norm_result {
        warn!(error = %e, "normalize failed");
    }
    pending_result?;
    norm_result
}

/// このエラーが起きたら残りの window を試行しても全て同じ理由で
/// 失敗する (= window 固有ではない) ものを判定する。
/// Db (checkpoint の記録自体ができない DB 障害) と
/// Spawn/EmptyCommand (bridge プロセスを起動できない環境障害)
/// が該当する。それ以外 (JV-Link が返すエラーコード、途中 EOF、
/// bridge の途中死等) は window 固有の可能性があるので続行対象とする。
fn is_systemic(e: &IngestError) -> bool {
    match e {
        IngestError::Db(_) => true,
        IngestError::Source(SourceError::Spawn(_) | SourceError::EmptyCommand) => true,
        IngestError::Source(_) => false,
    }
}

/// pending/failed の checkpoint を順に実行する。
/// dataspec が解釈できない行 (手動投入や古いテスト残骸) は warn して
/// 飛ばす。window 固有の失敗は checkpoint に記録されたあと収集し、
/// 後続の window を止めずに最後にまとめて報告する
/// (恒久的失敗が 1 件あっても他の window は進む)。
/// 系統的エラー (DB 断、bridge 起動不可) のみ即座に伝播する。
async fn run_pending(store: &Store, source: &dyn Source) -> Result<()> {
    // 一覧は 1 回だけ取得する。失敗した window は 'failed' のまま
    // pending 集合に残るため、反復ごとに取り直すと同じ window が
    // 先頭に戻ってきて無限リトライになる。
    let pending = store.pending_windows().await.into_diagnostic()?;
    let mut failures: Vec<String> = Vec::new();
    for ckpt in pending {
        let Ok(spec) = ckpt.dataspec.parse::<Dataspec>() else {
            warn!(dataspec = %ckpt.dataspec, window_start = %ckpt.window_start,
                "skipping checkpoint with unknown dataspec");
            continue;
        };
        let window = Window {
            dataspec: spec,
            start: ckpt.window_start.clone(),
            end: ckpt.window_end.clone(),
        };
        match run_window(store, source, &window, ckpt.last_file.as_deref()).await {
            Ok(()) => {}
            Err(e) if is_systemic(&e) => {
                return Err(miette::miette!(
                    "window {} {} failed: {e}",
                    spec.id(),
                    ckpt.window_start
                ));
            }
            Err(e) => {
                warn!(dataspec = %spec.id(), window_start = %ckpt.window_start,
                    error = %e, "window failed; continuing with remaining windows");
                failures.push(format!("{}@{}", spec.id(), ckpt.window_start));
            }
        }
    }
    if failures.is_empty() {
        info!("all windows done");
        Ok(())
    } else {
        Err(miette::miette!("windows failed: {}", failures.join(", ")))
    }
}

/// resume: 残り window + 各 dataspec の差分取得 + normalize。
///
/// 1 か所の失敗で後段を止めないよう、各段の失敗は分離する:
///   - pending window の失敗は checkpoint に記録済みなので、そのまま
///     diff/normalize に進み、最後にまとめて報告する
///   - diff は dataspec ごとに独立 (失敗しても cursor が進まないだけで
///     次回 resume が同じ位置から取り直す)
///   - normalize は commit 済み raw を滞留させないため必ず実行する
async fn resume(store: &Store, source: &dyn Source) -> Result<()> {
    let pending_result = run_pending(store, source).await;
    if let Err(e) = &pending_result {
        warn!(error = %e, "window failed; continuing with diff and normalize");
    }

    // setup が済んだ dataspec 全てについて cursor から差分を取る
    let mut diff_failures = Vec::new();
    for (spec_id, ts) in store.all_cursors().await.into_diagnostic()? {
        let Ok(spec) = spec_id.parse::<Dataspec>() else {
            warn!(dataspec = %spec_id, "skipping cursor with unknown dataspec");
            continue;
        };
        // MING/COMM は仕様書の組み合わせ表上 option=1 を指定できない
        // (セットアップ専用)。投げると -116 で必ず失敗するので除外する。
        if !spec.supports_normal_option() {
            warn!(dataspec = %spec_id, "dataspec does not support option=1; skipping diff");
            continue;
        }
        if let Err(e) = run_diff(store, source, spec, &ts).await {
            warn!(dataspec = %spec_id, error = %e, "diff fetch failed; continuing");
            diff_failures.push(spec_id);
            // spawn 不可・DB 断などの系統的エラーは残り dataspec も
            // 同じ結果なので打ち切る
            if is_systemic(&e) {
                break;
            }
        }
    }

    let norm_result = normalize_pending(store).await;
    if let Err(e) = &norm_result {
        warn!(error = %e, "normalize failed");
    }

    pending_result?;
    if !diff_failures.is_empty() {
        return Err(miette::miette!(
            "diff fetch failed for: {}",
            diff_failures.join(", ")
        ));
    }
    norm_result
}

/// raw_records の pending 分を jv.* へ反映する。
async fn normalize_pending(store: &Store) -> Result<()> {
    let s = store.normalize_pending().await.into_diagnostic()?;
    info!(
        normalized = s.normalized,
        parse_errors = s.parse_errors,
        skipped_unknown = s.skipped_unknown,
        "normalized pending raw records"
    );
    Ok(())
}

async fn status(store: &Store) -> Result<()> {
    let s = store.status().await.into_diagnostic()?;
    println!(
        "== runs      : running={} done={} failed={}",
        s.runs_running, s.runs_done, s.runs_failed
    );
    println!(
        "== windows   : pending={} done={} failed={} abandoned={}",
        s.windows_pending, s.windows_done, s.windows_failed, s.windows_abandoned
    );
    println!(
        "== raw       : records={} pending_parse={} parse_errors={}",
        s.raw_records, s.raw_pending_parse, s.parse_errors
    );
    println!(
        "== jv        : races={} entries={} horses={} payouts={}",
        s.jv_races, s.jv_entries, s.jv_horses, s.jv_payouts
    );
    if !s.current_runs.is_empty() {
        println!("== current runs");
        for (id, d, w, m) in &s.current_runs {
            println!("    #{id} {d:6} {w} {m}");
        }
    }
    if !s.cursors.is_empty() {
        println!("== cursors");
        for (d, ts) in &s.cursors {
            println!("    {d:6} {ts}");
        }
    }
    if !s.last_checkpoints.is_empty() {
        println!("== last checkpoints");
        for (d, w, st, f, last) in &s.last_checkpoints {
            println!(
                "    {d:6} {w} {st:7} files_done={f} last={:?}",
                last.as_deref().unwrap_or("-")
            );
        }
    }
    Ok(())
}
