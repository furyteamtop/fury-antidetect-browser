# Build context is the repository root (see docker-compose.yml), because the
# server depends on ../shared-rs through the workspace.

FROM rust:1-slim AS build
WORKDIR /build

RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev \
 && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml ./
COPY shared-rs/ shared-rs/
COPY server/ server/
# The agent is a workspace member but not needed in this image; give cargo a
# stub so the workspace still resolves.
RUN mkdir -p agent/src \
 && printf '[package]\nname = "fury-agent"\nversion.workspace = true\nedition.workspace = true\n' > agent/Cargo.toml \
 && echo 'fn main() {}' > agent/src/main.rs

RUN cargo build --release -p fury-server

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd -r -u 10001 fury

COPY --from=build /build/target/release/fury-server /usr/local/bin/fury-server
COPY server/migrations/ /app/migrations/

WORKDIR /app
USER fury
EXPOSE 8080
CMD ["fury-server"]
