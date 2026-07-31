# Two stages: the toolchain never reaches the running image.
FROM rust:1-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock ./
COPY shared-rs ./shared-rs
COPY server ./server
COPY agent ./agent
COPY tools ./tools
COPY shared ./shared
# Only the server. The agent and the desktop shell belong on an operator's
# machine, not on a box that holds other people's metadata.
RUN cargo build --release -p fury-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
# Not root: a container that only needs to listen and talk to Postgres has no
# business being able to write to its own filesystem as root.
RUN useradd --system --uid 10001 fury
COPY --from=build /src/target/release/fury-server /usr/local/bin/fury-server
USER fury
EXPOSE 8901
CMD ["fury-server"]
