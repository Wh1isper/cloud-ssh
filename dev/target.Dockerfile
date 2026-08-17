ARG OWLMUX_TARGET_BASE_IMAGE=debian:13-slim
FROM ${OWLMUX_TARGET_BASE_IMAGE}

ARG OWLMUX_TARGET_LOGIN_SHELL=/bin/bash

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates openssh-server tmux \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --shell "${OWLMUX_TARGET_LOGIN_SHELL}" owlmux \
    && install -d -m 0755 /run/sshd \
    && install -d -o owlmux -g owlmux -m 0700 /home/owlmux/.ssh

COPY dev/fixture/target-entrypoint.sh /usr/local/bin/owlmux-target-entrypoint

EXPOSE 22
ENTRYPOINT ["/usr/local/bin/owlmux-target-entrypoint"]
