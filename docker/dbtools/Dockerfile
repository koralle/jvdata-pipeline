# psql / pgcli を入れた運用ツール用イメージ。
# DB コンテナ (docker/postgres) に運用ツールを入れないために分離している。
FROM python:3.13-slim@sha256:9d2e5553305c7c7b0097999bb17187c69b921ccd6bc9d40e4bb5ebe652c00285

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        less \
        postgresql-client \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir pgcli
