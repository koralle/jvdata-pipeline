-- 年別レース数 (開催場別)
select extract(year from race_date) as year,
       jyo_cd,
       count(*) as races
from jv.races
group by 1, 2
order by 1, 2;

-- 年別・競馬場名つき (コードは JV-Data 仕様書の場コード)
select extract(year from r.race_date) as year,
       r.jyo_cd,
       count(distinct r.race_key) as races,
       count(e.umaban) as total_starters
from jv.races r
left join jv.entries e using (race_key)
group by 1, 2
order by 1, 2;
