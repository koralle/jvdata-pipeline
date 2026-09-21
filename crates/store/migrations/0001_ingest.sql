-- ingest 層: JV-Link から取り込んだ原データと実行状態を lossless に保持する。
--
-- runs        : 取得処理 1 回ごとの記録
-- checkpoints : window (dataspec × 期間) 単位の再開位置
-- cursors     : dataspec 単位の差分取得再開位置 (JVOpen lastfiletimestamp)
-- raw_records : JVGets が返したバイト列そのもの
-- parse_errors: raw → jv 正規化で失敗したレコードの記録

create schema if not exists ingest;
create schema if not exists jv;

create table ingest.runs (
    id                  bigint       generated always as identity primary key,
    dataspec            text         not null,
    -- 'setup' = option 4 (蓄積系初期取得), 'normal' = option 1 (差分)
    mode                text         not null check (mode in ('setup', 'normal')),
    -- JVOpen に渡した読み出しポイント時刻 (YYYYMMDDhhmmss)。end NULL = 終了時刻なし
    window_start        text         not null,
    window_end          text,
    jvopen_option       integer      not null,
    status              text         not null check (status in ('running', 'done', 'failed')),
    -- JVOpen が返した値
    read_count          integer,
    download_count      integer,
    last_file_timestamp text,
    files_done          integer      not null default 0,
    records_read        bigint       not null default 0,
    records_stored      bigint       not null default 0,
    records_skipped     bigint       not null default 0,
    parse_errors        bigint       not null default 0,
    error               text,
    started_at          timestamptz  not null default now(),
    finished_at         timestamptz
);

create index runs_status on ingest.runs (status);
create index runs_started on ingest.runs (started_at desc);

create table ingest.checkpoints (
    dataspec            text         not null,
    -- window の先頭 (setup の読み出し開始ポイント時刻 YYYYMMDDhhmmss)
    window_start        text         not null,
    window_end          text,
    mode                text         not null check (mode in ('setup', 'normal')),
    state               text         not null check (state in ('pending', 'running', 'done', 'failed')),
    -- commit 済みファイル数。中断後の再開時に JVSkip でこの数だけ読み飛ばす
    files_done          integer      not null default 0,
    -- 最後に commit したファイル名 (再開時の順序検証用)
    last_file           text,
    records_stored      bigint       not null default 0,
    last_file_timestamp text,
    last_run_id         bigint       references ingest.runs (id),
    error               text,
    updated_at          timestamptz  not null default now(),
    primary key (dataspec, window_start)
);

create index checkpoints_state on ingest.checkpoints (state);

-- 差分取得 (option 1) の再開位置。dataspec ごとに 1 行。
-- last_file_timestamp は JVOpen が返す「最終ファイルの提供時刻」。
create table ingest.cursors (
    dataspec            text         primary key,
    last_file_timestamp text         not null,
    updated_at          timestamptz  not null default now()
);

create table ingest.raw_records (
    id              bigint       generated always as identity primary key,
    run_id          bigint       not null references ingest.runs (id),
    -- run 内での連番
    seq             bigint       not null,
    -- JVGets が返した JV-Link ファイル名 (例: RASW20200101120000.jvd)
    filename        text         not null,
    -- ファイル内での 0 始まり連番
    file_seq        integer      not null,
    record_type     text         not null,
    data_kubun      text,
    make_date       date,
    -- JVGets が返したレコードデータのバイト列 (CRLF 含む)。decode しない原本。
    payload         bytea        not null,
    payload_sha256  bytea        not null,
    fetched_at      timestamptz  not null default now(),
    -- 0=pending, 1=normalized, 2=parse error, 3=unsupported type
    parse_state     smallint     not null default 0,
    -- 同一ファイルの同じ位置が同じ内容で再取得された場合の冪等キー。
    -- 訂正版は sha が変わるので別行として残り、normalize 側が後勝ちで適用する。
    unique (filename, file_seq, payload_sha256)
);

create index raw_records_pending on ingest.raw_records (id) where parse_state = 0;
create index raw_records_type on ingest.raw_records (record_type);
create index raw_records_filename on ingest.raw_records (filename);

create table ingest.parse_errors (
    id              bigint       generated always as identity primary key,
    raw_record_id   bigint       not null references ingest.raw_records (id),
    run_id          bigint       references ingest.runs (id),
    record_type     text,
    filename        text,
    error           text         not null,
    created_at      timestamptz  not null default now()
);

create index parse_errors_record on ingest.parse_errors (record_type);

-- 収集側ロール (wine コンテナ内プロセス / 将来の ingest 専用接続) への権限。
-- スキーマとテーブルは管理ロールが作るので、default privileges も合わせて張る。
grant usage on schema ingest, jv to jvdata_loader;
grant select, insert, update, delete on all tables in schema ingest to jvdata_loader;
grant select, insert, update, delete on all tables in schema jv to jvdata_loader;
grant usage, select on all sequences in schema ingest to jvdata_loader;
grant usage, select on all sequences in schema jv to jvdata_loader;
alter default privileges in schema ingest grant select, insert, update, delete on tables to jvdata_loader;
alter default privileges in schema jv grant select, insert, update, delete on tables to jvdata_loader;
alter default privileges in schema ingest grant usage, select on sequences to jvdata_loader;
alter default privileges in schema jv grant usage, select on sequences to jvdata_loader;
