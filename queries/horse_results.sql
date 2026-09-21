-- 馬ごとの出走回数・成績 (血統登録番号で馬名マスタと結合)
select coalesce(h.bamei, e.bamei) as bamei,
       e.ketto_num,
       count(*) as starts,
       count(*) filter (where e.kakutei_jyuni = 1) as wins,
       count(*) filter (where e.kakutei_jyuni <= 3) as top3,
       round(avg(e.kakutei_jyuni) filter (where e.kakutei_jyuni between 1 and 18), 2) as avg_jyuni
from jv.entries e
left join jv.horses h using (ketto_num)
where e.kakutei_jyuni is not null
group by 1, 2
order by wins desc, starts desc;

-- 騎手別の成績
select e.kisyu_ryakusyo as jockey,
       count(*) as starts,
       count(*) filter (where e.kakutei_jyuni = 1) as wins,
       round(100.0 * count(*) filter (where e.kakutei_jyuni = 1) / count(*), 1) as win_pct
from jv.entries e
where e.kakutei_jyuni is not null and e.kisyu_ryakusyo is not null
group by 1
order by wins desc;
