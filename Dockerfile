FROM node:24-bookworm-slim AS web-builder

WORKDIR /workspace
RUN npm install --global pnpm@11.20.0
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json apps/web/package.json
RUN pnpm install --filter @owlmux/web... --frozen-lockfile
COPY apps/web apps/web
RUN pnpm --filter @owlmux/web build

FROM rust:1.97-bookworm AS rust-builder

ARG OWLMUX_BUILD_REVISION=unknown
WORKDIR /workspace
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
RUN cargo build --release --locked --package owlmux-server

FROM debian:bookworm-slim AS runtime

ARG OWLMUX_VERSION=dev
ARG VCS_REF=unknown
ARG SOURCE_URL=https://github.com/owlfoundry/owlmux

LABEL org.opencontainers.image.title="OwlMux" \
      org.opencontainers.image.description="Self-hosted terminal roaming with target-owned tmux" \
      org.opencontainers.image.source="${SOURCE_URL}" \
      org.opencontainers.image.version="${OWLMUX_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.licenses="BSD-3-Clause"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl openssh-client tini \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 owlmux \
    && useradd --uid 10001 --gid owlmux --no-create-home --home-dir /nonexistent --shell /usr/sbin/nologin owlmux \
    && mkdir -p /usr/share/owlmux/web \
    && chown -R owlmux:owlmux /usr/share/owlmux

COPY --from=rust-builder --chown=owlmux:owlmux /workspace/target/release/owlmux-server /usr/local/bin/owlmux-server
COPY --from=web-builder --chown=owlmux:owlmux /workspace/apps/web/dist /usr/share/owlmux/web
COPY --chown=owlmux:owlmux LICENSE /usr/share/licenses/owlmux/LICENSE

USER owlmux
ENV OWLMUX_ADDR=0.0.0.0:8080 \
    OWLMUX_WEB_DIR=/usr/share/owlmux/web
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD curl --fail --silent --show-error http://127.0.0.1:8080/health >/dev/null || exit 1
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/owlmux-server"]
