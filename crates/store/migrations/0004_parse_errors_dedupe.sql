-- parse_errors を raw_record 単位で 1 行にする。
-- reparse で同じレコードが繰り返し失敗しても行が増殖しないよう、
-- 最新の失敗内容で upsert する (normalize.rs の ON CONFLICT が前提にする)。
delete from ingest.parse_errors a
using ingest.parse_errors b
where a.raw_record_id = b.raw_record_id
  and a.id < b.id;

create unique index parse_errors_raw_record on ingest.parse_errors (raw_record_id);
