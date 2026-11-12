#!/bin/sh
set -eu

REPO=${BOT_REPO:-}
if [ -z "$REPO" ]; then
  echo "Set BOT_REPO=owner/repository before running this installer." >&2
  exit 1
fi
case "$REPO" in */*) ;; *) echo "BOT_REPO must be owner/repository" >&2; exit 1 ;; esac
APP_DIR=${BOT_INSTALL_DIR:-/opt/tg-rust-support-bot}

if [ -f "$APP_DIR/.env" ]; then
  BOT_TOKEN=${BOT_TOKEN:-$(sed -n 's/^BOT_TOKEN=//p' "$APP_DIR/.env" | head -n 1)}
  BOT_OWNER_ID=${BOT_OWNER_ID:-$(sed -n 's/^BOT_OWNER_ID=//p' "$APP_DIR/.env" | head -n 1)}
fi

if [ "$(id -u)" -ne 0 ]; then
  echo "Run as root, for example: curl ... | sudo env BOT_REPO=$REPO sh" >&2
  exit 1
fi

if [ -z "${BOT_TOKEN:-}" ]; then
  printf 'Telegram bot token: ' > /dev/tty
  stty -echo < /dev/tty
  IFS= read -r BOT_TOKEN < /dev/tty
  stty echo < /dev/tty
  printf '\n' > /dev/tty
fi
if [ -z "${BOT_OWNER_ID:-}" ]; then
  printf 'Admin Telegram user ID [8883130864]: ' > /dev/tty
  IFS= read -r BOT_OWNER_ID < /dev/tty
  BOT_OWNER_ID=${BOT_OWNER_ID:-8883130864}
fi
case "$BOT_OWNER_ID" in ''|*[!0-9]*) echo "BOT_OWNER_ID must be numeric" >&2; exit 1 ;; esac

IMAGE="localhost/tg-rust-support-bot:installed"
mkdir -p "$APP_DIR/data"

ENGINE=
if command -v podman >/dev/null 2>&1 && podman info >/dev/null 2>&1; then
  ENGINE=$(command -v podman)
elif command -v docker >/dev/null 2>&1; then
  if command -v systemctl >/dev/null 2>&1; then
    systemctl enable --now docker >/dev/null 2>&1 || true
  fi
  if docker info >/dev/null 2>&1; then
    ENGINE=$(command -v docker)
  fi
fi

if [ -z "$ENGINE" ]; then
  if [ -r /etc/os-release ]; then
    . /etc/os-release
  fi
  case "${ID:-}:${ID_LIKE:-}" in
    ubuntu:*|debian:*|*:debian*)
      apt-get update
      apt-get install -y podman curl ca-certificates
      ;;
    fedora:*|rhel:*|centos:*|rocky:*|almalinux:*|*:fedora*|*:rhel*)
      if command -v dnf >/dev/null 2>&1; then
        dnf install -y podman curl ca-certificates
      else
        yum install -y podman curl ca-certificates
      fi
      ;;
    opensuse*:*|sles:*|*:suse*)
      zypper --non-interactive install podman curl ca-certificates
      ;;
    alpine:*)
      apk add --no-cache podman curl ca-certificates
      ;;
    *)
      echo "Unsupported Linux distribution. Install Podman or Docker, then rerun this command." >&2
      exit 1
      ;;
  esac
  if command -v podman >/dev/null 2>&1 && podman info >/dev/null 2>&1; then
    ENGINE=$(command -v podman)
  elif command -v docker >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
    ENGINE=$(command -v docker)
  else
    echo "Podman was installed but could not start. Install Docker or check the host service." >&2
    exit 1
  fi
fi

if [ "$(uname -m)" != x86_64 ]; then
  echo "The ready-to-install release binary is currently published for x86_64 Linux." >&2
  exit 1
fi

ASSET=tg-rust-support-bot-linux-x86_64
DOWNLOAD_URL="https://github.com/$REPO/releases/latest/download/$ASSET"
mkdir -p "$APP_DIR/image"
if command -v curl >/dev/null 2>&1; then
  curl -fL "$DOWNLOAD_URL" -o "$APP_DIR/image/bot"
elif command -v wget >/dev/null 2>&1; then
  wget -O "$APP_DIR/image/bot" "$DOWNLOAD_URL"
else
  echo "Install curl or wget to download the release binary." >&2
  exit 1
fi
chmod 755 "$APP_DIR/image/bot"
cat > "$APP_DIR/image/Dockerfile" <<'DOCKERFILE'
FROM alpine:3.20
RUN apk add --no-cache ca-certificates && addgroup -S -g 10001 bot && adduser -S -D -H -u 10001 -G bot bot
WORKDIR /app
COPY bot /usr/local/bin/bot
USER 10001:10001
ENTRYPOINT ["/usr/local/bin/bot"]
DOCKERFILE
"$ENGINE" build -t "$IMAGE" "$APP_DIR/image"

if [ ! -f "$APP_DIR/.env" ]; then
  umask 077
  cat > "$APP_DIR/.env" <<ENV
BOT_TOKEN=$BOT_TOKEN
ADMIN_GROUP_ID=
BOT_OWNER_ID=$BOT_OWNER_ID
DATABASE_URL=sqlite:///app/data/bot.db?mode=rwc
RATE_LIMIT_MAX_MSG=5
RATE_LIMIT_WINDOW_SEC=10
CAPTCHA_ENABLED=true
WORK_HOURS_ENABLED=false
WORK_HOURS_START=09:00
WORK_HOURS_END=18:00
OUT_OF_HOURS_TEXT=Мы ответим вам в ближайшее рабочее время.
ENV
fi

chown -R 10001:10001 "$APP_DIR/data"
chmod 600 "$APP_DIR/.env"
chmod 750 "$APP_DIR/data"

if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  cat > /etc/systemd/system/tg-rust-support-bot.service <<UNIT
[Unit]
Description=Telegram Support & Feedback Bot (container)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStartPre=-$ENGINE rm --force tg-rust-support-bot
ExecStart=$ENGINE run --rm --name tg-rust-support-bot --env-file $APP_DIR/.env --volume $APP_DIR/data:/app/data:Z $IMAGE
ExecStop=-$ENGINE stop --time 20 tg-rust-support-bot
Restart=always
RestartSec=5
TimeoutStopSec=30

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable tg-rust-support-bot.service
  if systemctl is-active --quiet tg-rust-support-bot.service; then
    systemctl restart tg-rust-support-bot.service
  else
    systemctl start tg-rust-support-bot.service
  fi
  echo "Installed using $ENGINE. Logs: journalctl -u tg-rust-support-bot -f"
else
  "$ENGINE" rm --force tg-rust-support-bot >/dev/null 2>&1 || true
  "$ENGINE" run --detach --restart=unless-stopped --name tg-rust-support-bot --env-file "$APP_DIR/.env" --volume "$APP_DIR/data:/app/data:Z" "$IMAGE"
  echo "Installed using $ENGINE. Container: tg-rust-support-bot"
fi
