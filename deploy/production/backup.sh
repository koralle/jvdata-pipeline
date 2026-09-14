#!/bin/sh
# このホストの PostgreSQL のバックアップを取る。
#
#   毎日 pg_dump (custom 形式)          -> /backup/dump/
#   毎週 pg_basebackup (PITR の起点)    -> /backup/base/
#
# 同じホストのボリュームに置くだけなので、別ホストへの退避は別途必要 (README 参照)。
# ここが埋まると DB も止まるので容量を監視すること。
#
# 最終実行の時刻は /backup/.last-dump / /backup/.last-base に残す。
# ボリュームにあるのでコンテナを再起動しても間隔が維持される
# (restart: unless-stopped のたびに全量 pg_basebackup が走るのを防ぐ)。
# マーカーが無い初回だけ両方を即実行する。
#
# 1 回だけ走らせて確認するときは BACKUP_RUN_ONCE=1 を付ける。
set -eu

BACKUP_ROOT="${BACKUP_DIR:-/backup}"
DUMP_DIR="$BACKUP_ROOT/dump"
BASE_DIR="$BACKUP_ROOT/base"
KEEP_DUMPS="${BACKUP_KEEP_DUMPS:-7}"
KEEP_BASE="${BACKUP_KEEP_BASE:-2}"
DUMP_INTERVAL="${BACKUP_DUMP_INTERVAL_SECONDS:-86400}"
BASE_INTERVAL="${BACKUP_BASE_INTERVAL_SECONDS:-604800}"

LAST_DUMP="$BACKUP_ROOT/.last-dump"
LAST_BASE="$BACKUP_ROOT/.last-base"

prune() {
  dir="$1"
  keep="$2"
  # ファイル名は $POSTGRES_DB-$stamp.dump / $stamp だけなので ls -t で足りる
  # shellcheck disable=SC2012
  ls -1dt "$dir"/* 2>/dev/null | tail -n "+$((keep + 1))" | while read -r old; do
    echo "backup: removing $old"
    rm -rf "$old"
  done
}

dump() {
  mkdir -p "$DUMP_DIR"
  stamp="$(date +%Y%m%d-%H%M%S)"
  file="$DUMP_DIR/$POSTGRES_DB-$stamp.dump"
  echo "backup: pg_dump -> $file"
  # || で呼ばれた関数内では set -e が効かないので明示的に return する
  pg_dump --host=postgres --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
    --format=custom --file="$file" || return 1
  date +%s > "$LAST_DUMP"
  prune "$DUMP_DIR" "$KEEP_DUMPS"
}

base() {
  mkdir -p "$BASE_DIR"
  stamp="$(date +%Y%m%d-%H%M%S)"
  dir="$BASE_DIR/$stamp"
  echo "backup: pg_basebackup -> $dir"
  pg_basebackup --host=postgres --username="$POSTGRES_USER" \
    --pgdata="$dir" --format=tar --gzip --wal-method=stream --checkpoint=fast \
    || return 1
  date +%s > "$LAST_BASE"
  prune "$BASE_DIR" "$KEEP_BASE"
}

# マーカーの時刻 + 間隔を次回実行時刻とする。マーカーが無ければ今すぐ。
next_run() {
  marker="$1"
  interval="$2"
  last=0
  if [ -f "$marker" ]; then
    last="$(cat "$marker" 2>/dev/null || echo 0)"
  fi
  case "$last" in
    ''|*[!0-9]*) last=0 ;;
  esac
  if [ "$last" -gt 0 ]; then
    echo $((last + interval))
  else
    date +%s
  fi
}

if [ "${BACKUP_RUN_ONCE:-0}" = "1" ]; then
  dump
  base
  exit 0
fi

next_dump="$(next_run "$LAST_DUMP" "$DUMP_INTERVAL")"
next_base="$(next_run "$LAST_BASE" "$BASE_INTERVAL")"

while true; do
  now="$(date +%s)"
  if [ "$now" -ge "$next_dump" ]; then
    dump || echo "backup: dump failed" >&2
    next_dump=$((now + DUMP_INTERVAL))
  fi
  if [ "$now" -ge "$next_base" ]; then
    base || echo "backup: base backup failed" >&2
    next_base=$((now + BASE_INTERVAL))
  fi

  # 近い方の予定時刻まで寝る (下限 60 秒)
  wake="$next_dump"
  if [ "$next_base" -lt "$wake" ]; then
    wake="$next_base"
  fi
  now="$(date +%s)"
  sleep_for=$((wake - now))
  if [ "$sleep_for" -lt 60 ]; then
    sleep_for=60
  fi
  sleep "$sleep_for"
done
