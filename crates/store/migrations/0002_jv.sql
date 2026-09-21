-- jv 層: JV-Data を分析しやすい形に正規化したテーブル群。
--
-- minimum slice: races / entries / horses / jockeys / trainers / payouts / schedule_days
--
-- 方針:
--   - 外部キーは張らない。raw → jv の反映は「後から来たレコードが勝つ」
--     ( fetched 順に適用 ) で、SE が RA より先に来ても壊れない。
--   - コード値は JRA-VAN 仕様上の文字列のまま保持する (0 埋め込み)。
--   - 数値系 (オッズ・負担重量) は numeric で保持する。

create table jv.races (
    -- YYYYMMDD + JyoCD + Kaiji + Nichiji + RaceNum (16 chars)
    race_key        text        primary key,
    race_date       date,
    jyo_cd          text        not null,
    kaiji           smallint,
    nichiji         smallint,
    race_num        smallint,
    youbi_cd        text,
    toku_num        integer,
    hondai          text,
    fukudai         text,
    kakko           text,
    ryakusyo10      text,
    ryakusyo6       text,
    ryakusyo3       text,
    grade_cd        text,
    syubetu_cd      text,
    kigo_cd         text,
    jyuryo_cd       text,
    jyoken_name     text,
    kyori           integer,
    track_cd        text,
    course_kubun_cd text,
    hasso_time      text,
    toroku_tosu     smallint,
    syusso_tosu     smallint,
    nyusen_tosu     smallint,
    tenko_cd        text,
    siba_baba_cd    text,
    dirt_baba_cd    text,
    data_kubun      text,
    make_date       date,
    source_file     text,
    updated_at      timestamptz not null default now()
);

create index races_date on jv.races (race_date);
create index races_jyo on jv.races (jyo_cd, race_date);

create table jv.entries (
    race_key            text      not null,
    umaban              text      not null,
    wakuban             smallint,
    ketto_num           text,
    bamei               text,
    uma_kigo_cd         text,
    sex_cd              text,
    hinsyu_cd           text,
    keiro_cd            text,
    barei               smallint,
    tozai_cd            text,
    chokyosi_code       text,
    chokyosi_ryakusyo   text,
    banusi_code         text,
    banusi_name         text,
    -- 負担重量 kg (元データ 0.1kg 単位 → numeric(4,1))
    futan               numeric(4, 1),
    blinker             text,
    kisyu_code          text,
    kisyu_ryakusyo      text,
    minarai_cd          text,
    ba_taijyu           smallint,
    zogen_sa            smallint,
    ijyo_cd             text,
    nyusen_jyuni        smallint,
    kakutei_jyuni       smallint,
    dochaku_kubun       text,
    -- 走破タイム (秒)。元データは "9分99秒9" (分.秒.1/10) の
    -- packed 形式を decode 済み (例: "1335" → 93.5)。
    time_sec            numeric(6, 1),
    chakusa_cd          text,
    jyuni_1c            smallint,
    jyuni_2c            smallint,
    jyuni_3c            smallint,
    jyuni_4c            smallint,
    -- 単勝オッズ (元データ 0.1 倍単位 → numeric(6,1))
    odds                numeric(6, 1),
    ninki               smallint,
    honsyokin           bigint,
    fukasyokin          bigint,
    -- 後 3 ハロン (秒)。元データは "99秒9" (0.1 秒単位) の整数。
    haron_l3_sec        numeric(4, 1),
    data_kubun          text,
    make_date           date,
    source_file         text,
    updated_at          timestamptz not null default now(),
    primary key (race_key, umaban)
);

create index entries_ketto on jv.entries (ketto_num);
create index entries_kisyu on jv.entries (kisyu_code);
create index entries_jyuni on jv.entries (kakutei_jyuni);

create table jv.horses (
    ketto_num           text      primary key,
    del_kubun           text,
    reg_date            date,
    del_date            date,
    birth_date          date,
    bamei               text,
    bamei_kana          text,
    bamei_eng           text,
    sex_cd              text,
    hinsyu_cd           text,
    keiro_cd            text,
    sire_name           text,
    dam_name            text,
    tozai_cd            text,
    chokyosi_code       text,
    chokyosi_ryakusyo   text,
    breeder_code        text,
    breeder_name        text,
    sanchi_name         text,
    banusi_code         text,
    banusi_name         text,
    race_count          integer,
    updated_at          timestamptz not null default now()
);

create index horses_bamei on jv.horses (bamei);

create table jv.jockeys (
    kisyu_code          text      primary key,
    del_kubun           text,
    kisyu_name          text,
    kisyu_name_kana     text,
    kisyu_ryakusyo      text,
    kisyu_name_eng      text,
    sex_cd              text,
    minarai_cd          text,
    tozai_cd            text,
    chokyosi_code       text,
    updated_at          timestamptz not null default now()
);

create table jv.trainers (
    chokyosi_code       text      primary key,
    del_kubun           text,
    chokyosi_name       text,
    chokyosi_name_kana  text,
    chokyosi_ryakusyo   text,
    sex_cd              text,
    tozai_cd            text,
    updated_at          timestamptz not null default now()
);

create table jv.payouts (
    race_key    text      not null,
    -- tansho / fukusyo / wakuren / umaren / wide / umatan / sanrenpuku / sanrentan
    bet_type    text      not null,
    -- 馬番 or 組番 (JRA-VAN の組番表現をそのまま保持)
    combo       text      not null,
    pay         integer   not null,
    ninki       integer,
    primary key (race_key, bet_type, combo)
);

create index payouts_race on jv.payouts (race_key);

create table jv.schedule_days (
    race_date   date      not null,
    jyo_cd      text      not null,
    kaiji       smallint,
    nichiji     smallint,
    youbi_cd    text,
    updated_at  timestamptz not null default now(),
    primary key (race_date, jyo_cd)
);

-- 権限は 0001_ingest.sql の default privileges で ingest/jv 両スキーマに済んでいる。
