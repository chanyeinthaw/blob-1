FROM node:22-bookworm AS web-build
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY . .
COPY --from=web-build /src/web/dist ./web/dist
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p agent && cp target/release/agent /usr/local/bin/agent

FROM ubuntu:24.04
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl file git python3 ripgrep \
    && rm -rf /var/lib/apt/lists/*
# Node.js LTS (with npm/npx/corepack) from the official image used by web-build.
COPY --from=web-build /usr/local /usr/local
# uv/uvx for Python-based MCP servers.
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /usr/local/bin/
COPY --from=build /usr/local/bin/agent /usr/local/bin/agent
ENV XDG_DATA_HOME=/data
VOLUME /data
ENTRYPOINT ["agent"]
CMD ["serve"]
