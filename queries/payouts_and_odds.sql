-- 人気別の勝率・単勝回収率 (haron/odds は元データの 0.1 倍を numeric に正規化済み)
select e.ninki,
       count(*) as starters,
       count(*) filter (where e.kakutei_jyuni = 1) as wins,
       round(100.0 * count(*) filter (where e.kakutei_jyuni = 1) / count(*), 1) as win_pct,
       round(avg(p.pay) filter (where e.kakutei_jyuni = 1)) as avg_tansyo_pay
from jv.entries e
left join jv.payouts p
       on p.race_key = e.race_key and p.bet_type = 'tansho'
      and p.combo = lpad(e.umaban, 2, '0')
where e.ninki between 1 and 18 and e.kakutei_jyuni is not null
group by 1
order by 1;

-- レース別 払戻一覧
select r.race_date, r.hondai, p.bet_type, p.combo, p.pay
from jv.payouts p
join jv.races r using (race_key)
order by 1, 2, p.bet_type;
