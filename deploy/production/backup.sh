#!/bin/sh
# このホストの PostgreSQL のバックアップを取る。
#
#   毎日 pg_dump (custom 形式)          -> /backup/dump/
#   毎週 pg_basebackup (PITR の起点)    -> /backup/base/
#
# WAL アーカイブは、残っているベースバックアップのうち最古のものの START WAL
# より前の分を pg_archivecleanup で消す (ベースが取れていない間は境が進まない
# ので、アーカイブだけ先に消えて PITR が壊れることはない)。
#
# 同じホストのボリュームに置くだけなので、別ホストへの退避は別途必要 (README 参照)。
# ここが埋まると DB も止まるので容量を監視すること。
#
# 1 回だけ走らせて確認するときは BACKUP_RUN_ONCE=1 を付ける。
set -eu

BACKUP_ROOT="${BACKUP_DIR:-/backup}"
DUMP_DIR="$BACKUP_ROOT/dump"
BASE_DIR="$BACKUP_ROOT/base"
WAL_DIR="${BACKUP_WAL_DIR:-/var/lib/postgresql/wal-archive}"
KEEP_DUMPS="${BACKUP_KEEP_DUMPS:-7}"
KEEP_BASE="${BACKUP_KEEP_BASE:-2}"
DUMP_INTERVAL="${BACKUP_DUMP_INTERVAL_SECONDS:-86400}"
BASE_INTERVAL="${BACKUP_BASE_INTERVAL_SECONDS:-604800}"

prune() {
  dir="$1"
  keep="$2"
  # .incomplete は残っていても世代として数えない (中断した残骸は含めない)
  ls -1dt "$dir"/* 2>/dev/null | grep -v '\.incomplete$' | tail -n +$((keep + 1)) | while read -r old; do
    echo "backup: removing $old"
    rm -rf "$old"
  done
}

# 残っているベースバックアップのうち最古のものより前の WAL は、そのベースを
# 起点にしても replay できない (PITR はベースの START WAL からしか進めない)
# ので、境のファイル名を pg_archivecleanup に渡して消す。最古のベースが
# 変わらない限り境は進まないので、ベースが失敗し続けても WAL だけ先に
# 消えることはない。
prune_wal() {
  oldest="$(ls -1dt "$BASE_DIR"/* 2>/dev/null | grep -v '\.incomplete$' | tail -n 1)"
  if [ -z "$oldest" ]; then
    return 0
  fi
  # 境は base.tar.gz 内の backup_label が持っている:
  #   START WAL LOCATION: 0/3000028 (file 000000010000000000000003)
  # メンバー名の書き方 (backup_label / ./backup_label) は実装で揺れるので、
  # 一覧から拾ってから取り出す
  label_member="$(tar -tzf "$oldest/base.tar.gz" 2>/dev/null | sed -n '/backup_label$/p' | head -n 1)"
  if [ -n "$label_member" ]; then
    start_wal="$(tar -xzf "$oldest/base.tar.gz" -O "$label_member" 2>/dev/null \
      | sed -n 's/^START WAL LOCATION: .*file \([0-9A-Fa-f]\{24\}\).*/\1/p')"
  else
    start_wal=""
  fi
  if [ -z "$start_wal" ]; then
    echo "backup: cannot read START WAL from $oldest/base.tar.gz; keeping WAL archive" >&2
    return 0
  fi
  echo "backup: pruning WAL archive before $start_wal"
  pg_archivecleanup "$WAL_DIR" "$start_wal"
}

# dump / base は呼び出し側で `|| echo` / `if` と組んで呼ばれるので、関数の中では
# set -e が効かない (POSIX の仕様。|| の左辺や if の条件では errexit が抑制される)。
# 失敗したらそこで return しないと、そのまま prune まで進んで健全な世代を
# 消してしまう。確定名に直接書くと、失敗した分が最新世代として prune の
# 枠を食うので、.incomplete に書いて成功してから mv する。

dump() {
  mkdir -p "$DUMP_DIR"
  stamp="$(date +%Y%m%d-%H%M%S)"
  file="$DUMP_DIR/$POSTGRES_DB-$stamp.dump"
  echo "backup: pg_dump -> $file"
  if ! pg_dump --host=postgres --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
    --format=custom --file="$file.incomplete"; then
    rm -f "$file.incomplete"
    return 1
  fi
  mv "$file.incomplete" "$file" || return 1
  prune "$DUMP_DIR" "$KEEP_DUMPS"
}

base() {
  mkdir -p "$BASE_DIR"
  stamp="$(date +%Y%m%d-%H%M%S)"
  dir="$BASE_DIR/$stamp"
  echo "backup: pg_basebackup -> $dir"
  if ! pg_basebackup --host=postgres --username="$POSTGRES_USER" \
    --pgdata="$dir.incomplete" --format=tar --gzip --wal-method=stream --checkpoint=fast; then
    rm -rf "$dir.incomplete"
    return 1
  fi
  mv "$dir.incomplete" "$dir" || return 1
  prune "$BASE_DIR" "$KEEP_BASE"
  # WAL の世代管理に失敗してもベースバックアップ自体はできているので、
  # ここだけの失敗で base を失敗扱いにはしない
  prune_wal || echo "backup: WAL archive prune failed" >&2
}

if [ "${BACKUP_RUN_ONCE:-0}" = "1" ]; then
  dump
  base
  exit 0
fi

next_base="$(date +%s)"
while true; do
  dump || echo "backup: dump failed" >&2
  now="$(date +%s)"
  if [ "$now" -ge "$next_base" ]; then
    if base; then
      next_base=$((now + BASE_INTERVAL))
    else
      echo "backup: base backup failed" >&2
      # 失敗したまま 1 周期 (1 週間) 待たず、次の dump 周期でやり直す
      next_base=$((now + DUMP_INTERVAL))
    fi
  fi
  sleep "$DUMP_INTERVAL"
done
