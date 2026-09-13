# 32-bit Windows COM コンポーネント (JV-Link) を Wine 経由で呼ぶためのイメージ。
#
# JV-Link は 32-bit x86 のバイナリなので、このイメージは x86_64 ホストでしか
# 意味を持たない。ARM ホストでは起動時のスモークテストで落ちる (fail closed)。

# 32-bit の COM スモークテストを mingw-w64 でビルドする段。
FROM ubuntu:24.04 AS com32-builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        binutils-mingw-w64-i686 \
        gcc-mingw-w64-i686 \
    && rm -rf /var/lib/apt/lists/*

COPY smoke/com32.c /build/com32.c
RUN i686-w64-mingw32-gcc \
        -std=c99 \
        -O2 \
        -Wall \
        -Wextra \
        -static \
        -Wl,--no-insert-timestamp \
        -o /com32.exe \
        /build/com32.c \
        -lole32


FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
ENV WINEPREFIX=/wineprefix
# win64 prefix は 64-bit と 32-bit の両方を持つ。32-bit の JV-Link は
# その 32-bit 側 (drive_c/windows/syswow64) で動く。
ENV WINEARCH=win64
ENV DISPLAY=:1

RUN dpkg --add-architecture i386 \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        fluxbox \
        fonts-noto-cjk \
        gpg \
        locales \
        novnc \
        tini \
        tzdata \
        websockify \
        x11vnc \
        xvfb \
    && mkdir -pm755 /etc/apt/keyrings \
    && curl -fsSL https://dl.winehq.org/wine-builds/winehq.key \
        | gpg --dearmor -o /etc/apt/keyrings/winehq-archive.key - \
    && curl -fsSL -o /etc/apt/sources.list.d/winehq-noble.sources \
        https://dl.winehq.org/wine-builds/ubuntu/dists/noble/winehq-noble.sources \
    && apt-get update \
    && apt-get install -y --install-recommends winehq-stable \
    && locale-gen ja_JP.UTF-8 \
    && rm -rf /var/lib/apt/lists/*

# Wine は LANG から ANSI コードページを決める。JV-Link が扱うのは Shift_JIS
# (CP932) なので、ロケールを生成するだけでなく ja_JP.UTF-8 を既定にしておく。
ENV LANG=ja_JP.UTF-8
ENV LC_ALL=ja_JP.UTF-8
ENV TZ=Asia/Tokyo

COPY --from=com32-builder /com32.exe /opt/smoke/com32.exe
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

EXPOSE 6080

# Wine は wineserver / winedevice.exe などの子プロセスを残すので、
# PID 1 に tini を置かないとゾンビが溜まる。
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
CMD ["sleep", "infinity"]
