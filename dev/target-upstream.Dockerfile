FROM debian:13-slim AS tmux-build

ARG OWLMUX_TMUX_VERSION=3.7b
ARG OWLMUX_TMUX_SHA256=87f2e99e3b685973f2ca002ffd6ed7e51a5744f7009daae5a15670b6d532db96

RUN apt-get update \
    && apt-get install --yes --no-install-recommends bison build-essential ca-certificates curl libevent-dev libncurses-dev pkg-config \
    && curl --fail --location --silent --show-error \
      "https://github.com/tmux/tmux/releases/download/${OWLMUX_TMUX_VERSION}/tmux-${OWLMUX_TMUX_VERSION}.tar.gz" \
      --output /tmp/tmux.tar.gz \
    && echo "${OWLMUX_TMUX_SHA256}  /tmp/tmux.tar.gz" | sha256sum --check --strict \
    && mkdir /tmp/tmux \
    && tar --extract --gzip --file /tmp/tmux.tar.gz --directory /tmp/tmux --strip-components=1 \
    && cd /tmp/tmux \
    && ./configure --prefix=/usr \
    && make --jobs="$(nproc)" \
    && make DESTDIR=/out install

FROM debian:13-slim

ARG OWLMUX_TARGET_LOGIN_SHELL=/bin/bash

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates libevent-core-2.1-7t64 libncursesw6 openssh-server \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell "${OWLMUX_TARGET_LOGIN_SHELL}" owlmux \
    && install -d -m 0755 /run/sshd \
    && install -d -o owlmux -g owlmux -m 0700 /home/owlmux/.ssh

COPY --from=tmux-build /out/usr/bin/tmux /usr/bin/tmux
COPY dev/fixture/target-entrypoint.sh /usr/local/bin/owlmux-target-entrypoint

EXPOSE 22
ENTRYPOINT ["/usr/local/bin/owlmux-target-entrypoint"]
