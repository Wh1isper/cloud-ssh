#!/bin/sh
set -eu

ssh-keygen -A
install -d -o owlmux -g owlmux -m 0700 /home/owlmux/.ssh
if [ ! -e /home/owlmux/.ssh/authorized_keys ]; then
    install -o owlmux -g owlmux -m 0600 /dev/null /home/owlmux/.ssh/authorized_keys
fi
chown owlmux:owlmux /home/owlmux/.ssh/authorized_keys
chmod 0600 /home/owlmux/.ssh/authorized_keys

cat >/etc/ssh/sshd_config.d/owlmux-target.conf <<'EOF'
ListenAddress 127.0.0.1
Port 22
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin no
PubkeyAuthentication yes
AllowAgentForwarding no
AllowTcpForwarding no
GatewayPorts no
X11Forwarding no
PermitTunnel no
PermitUserEnvironment no
AllowUsers owlmux
LogLevel VERBOSE
EOF

if ! su - owlmux -c '/usr/bin/tmux -L owlmux has-session -t alpha 2>/dev/null'; then
    su - owlmux -c '/usr/bin/tmux -L owlmux new-session -d -s alpha "exec sh -c '\''printf \"primary-ready\377\"; while :; do if [ -f /tmp/owlmux-live-output ]; then cat /tmp/owlmux-live-output; rm -f /tmp/owlmux-live-output; fi; sleep 1; done'\''"'
    su - owlmux -c '/usr/bin/tmux -L owlmux split-window -d -t alpha:0 "exec sh -c '\''printf secondary-ready; while :; do sleep 3600; done'\''"'
    su - owlmux -c '/usr/bin/tmux -L owlmux select-layout -t alpha:0 even-horizontal'
fi

exec /usr/sbin/sshd -D -e
