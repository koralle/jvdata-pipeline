-- jv.entries に増減符号 (ZogenFugo) を追加。
--
-- SE レコードの馬体重増減は「増減符号 (1B) + 増減差 (3B)」の 2 項目。
-- 符号カラムが無いと +6kg と -6kg が区別できないため追加する。
alter table jv.entries add column zogen_fugo text;
