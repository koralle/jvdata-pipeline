-- jv.* の各行に「最後に適用した raw_records.id」を記録する。
--
-- raw id は取得順に単調増加するので版数として使える。
-- reparse で古いレコードを再適用しても、より新しい raw から
-- 適用済みの行は巻き戻らない (normalize.rs の upsert/delete ガードが
-- 前提にする)。既存行は NULL = 版不明 → どの raw よりも古い扱い。

alter table jv.races         add column applied_raw_id bigint;
alter table jv.entries       add column applied_raw_id bigint;
alter table jv.horses        add column applied_raw_id bigint;
alter table jv.jockeys       add column applied_raw_id bigint;
alter table jv.trainers      add column applied_raw_id bigint;
alter table jv.payouts       add column applied_raw_id bigint;
alter table jv.schedule_days add column applied_raw_id bigint;

-- data_kubun=0 (該当レコード削除) で消したエンティティの墓標。
-- 削除は行ごとの applied_raw_id では判定できない (行自体が無い) ので、
-- 「どの raw による削除が最後に効いたか」を別途持つ。
-- これより古い raw の upsert/delete は apply 時に捨てる
-- (古い reparse で削除済みデータが復活しない)。
create table jv.entity_tombstones (
    entity         text        not null,
    entity_key     text        not null,
    applied_raw_id bigint      not null,
    applied_at     timestamptz not null default now(),
    primary key (entity, entity_key)
);
