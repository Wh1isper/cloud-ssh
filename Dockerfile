FROM rust:1.80-slim-bookworm AS builder

WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN cargo build --release --locked -p cloud-ssh-server --bin cloud-ssh

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && useradd --system --uid 10001 --create-home --home-dir /home/cloudssh cloudssh \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/target/release/cloud-ssh /usr/local/bin/cloud-ssh

USER cloudssh

ENTRYPOINT ["/usr/local/bin/cloud-ssh"]
