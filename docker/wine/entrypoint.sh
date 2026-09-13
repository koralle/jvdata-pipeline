#!/usr/bin/env bash
# Wine prefix を準備し、noVNC のデスクトップを立ててから CMD を exec する。
#
# prefix の初期化は system.reg が無いときだけ。prefix はボリュームに載せて
# あるので、手作業で入れた JV-Link と利用登録は down / up では消えない。
set -euo pipefail

export DISPLAY="${DISPLAY:-:1}"
export WINEPREFIX="${WINEPREFIX:-/wineprefix}"
export WINEARCH="${WINEARCH:-win64}"

readonly VNC_PORT=5900
readonly NOVNC_PORT=6080
readonly SYSWOW64="$WINEPREFIX/drive_c/windows/syswow64"

# Xvfb の socket ができるまで待つ。待たずに WM と VNC を起動すると
# 繋がらないまま空の画面になる。
wait_for_display() {
  local socket="/tmp/.X11-unix/X${DISPLAY#:}"
  local i
  for ((i = 0; i < 100; i++)); do
    if [ -e "$socket" ]; then
      return 0
    fi
    sleep 0.1
  done
  echo "Xvfb did not come up on $DISPLAY" >&2
  return 1
}

start_display() {
  rm -f "/tmp/.X${DISPLAY#:}-lock"

  Xvfb "$DISPLAY" -screen 0 "${XVFB_SCREEN:-1280x800x24}" -nolisten tcp \
    >/tmp/xvfb.log 2>&1 &
  wait_for_display

  fluxbox >/tmp/fluxbox.log 2>&1 &
  x11vnc -display "$DISPLAY" -forever -shared -rfbport "$VNC_PORT" -nopw \
    >/tmp/x11vnc.log 2>&1 &
  websockify --web=/usr/share/novnc "0.0.0.0:$NOVNC_PORT" "localhost:$VNC_PORT" \
    >/tmp/novnc.log 2>&1 &
}

init_wine() {
  if [ ! -f "$WINEPREFIX/system.reg" ]; then
    echo "wine: creating prefix $WINEPREFIX"
    # wineboot に X の表示を見せてはいけない。見せると win64 prefix の
    # 32-bit 側が作られず syswow64 が空同然になり (実測 846 -> 1 ファイル)、
    # 32-bit の JV-Link が応答も stderr も返さずに固まる。
    env -u DISPLAY WINEDLLOVERRIDES="mscoree,mshtml=" wineboot --init
    wineserver -w
  fi

  # 壊れた prefix でも Wine 自体は起動する。ここで止めないと、失敗は
  # 取得時の不可解なタイムアウトとしてしか現れない。
  if [ ! -f "$SYSWOW64/kernel32.dll" ]; then
    cat >&2 <<EOF
wine: $WINEPREFIX has no 32-bit support (missing $SYSWOW64/kernel32.dll).
wine: JV-Link and every 32-bit process that calls it would hang without output.
wine: On a non-x86_64 host there is no way to run 32-bit x86 under Wine at all;
wine: remove the wineprefix volume and start over on an x86_64 host.
EOF
    return 1
  fi
}

# 32-bit の実行体が実際に走るかまで確かめる。ファイルの有無だけでは、
# 起動しない 32-bit プロセスを見逃す。
smoke_test() {
  # エミュレーションの下では冷えた起動が分単位になることがあるので、
  # 無応答のまま up が止まらないよう上限を切る。
  if ! timeout 60 wine /opt/smoke/com32.exe; then
    cat >&2 <<'EOF'
wine: the 32-bit COM smoke test failed.
wine: This host cannot run 32-bit x86 Windows binaries under Wine.
wine: Use an x86_64 host; see README.md.
EOF
    return 1
  fi
}

report() {
  cat <<EOF
wine desktop: http://localhost:${NOVNC_PORT}/vnc.html
wine prefix:  $WINEPREFIX
EOF

  if [ ! -f "$SYSWOW64/JVDTLAB/JVDTLab.dll" ]; then
    cat <<'EOF'
wine: JV-Link is not installed in this prefix yet.
wine: Install it once by hand on the noVNC desktop; see README.md.
EOF
  fi
}

mkdir -p "$WINEPREFIX"

start_display
init_wine
smoke_test
report

exec "$@"
