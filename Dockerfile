FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p agent && cp target/release/agent /usr/local/bin/agent

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates curl git ripgrep \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/agent /usr/local/bin/agent
ENV XDG_DATA_HOME=/data
VOLUME /data
ENTRYPOINT ["agent"]
CMD ["serve"]
