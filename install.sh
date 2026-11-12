#!/bin/sh
set -eu

REPO=${BOT_REPO:-}
if [ -z "$REPO" ]; then
  echo "Set BOT_REPO=owner/repository before running this installer." >&2
  exit 1
fi
case "$REPO" in */*) ;; *) echo "BOT_REPO must be owner/repository" >&2; exit 1 ;; esac

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
ARCH=$(uname -m)
if [ "$ARCH" != x86_64 ]; then
  echo "No standalone binary is published for $ARCH yet. Use Docker Compose on this architecture." >&2
  exit 1
fi

APP_DIR=${BOT_INSTALL_DIR:-/opt/tg-rust-support-bot}
ASSET=tg-rust-support-bot-linux-x86_64
mkdir -p "$APP_DIR/data"
DOWNLOAD_URL="https://github.com/$REPO/releases/latest/download/$ASSET"
if command -v curl >/dev/null 2>&1; then
  curl -fL "$DOWNLOAD_URL" -o "$APP_DIR/bot.new"
elif command -v wget >/dev/null 2>&1; then
  wget -O "$APP_DIR/bot.new" "$DOWNLOAD_URL"
else
  echo "Install curl or wget first." >&2
  exit 1
fi
chmod 755 "$APP_DIR/bot.new"
mv "$APP_DIR/bot.new" "$APP_DIR/bot"

if [ ! -f "$APP_DIR/.env" ]; then
  umask 077
  cat > "$APP_DIR/.env" <<ENV
BOT_TOKEN=$BOT_TOKEN
ADMIN_GROUP_ID=
BOT_OWNER_ID=$BOT_OWNER_ID
DATABASE_URL=sqlite://$APP_DIR/data/bot.db?mode=rwc
RATE_LIMIT_MAX_MSG=5
RATE_LIMIT_WINDOW_SEC=10
CAPTCHA_ENABLED=true
WORK_HOURS_ENABLED=false
WORK_HOURS_START=09:00
WORK_HOURS_END=18:00
OUT_OF_HOURS_TEXT=Мы ответим вам в ближайшее рабочее время.
ENV
fi

RUN_USER=root
if command -v useradd >/dev/null 2>&1; then
  if ! id tg-support-bot >/dev/null 2>&1; then
    useradd --system --home-dir "$APP_DIR" --shell /usr/sbin/nologin tg-support-bot
  fi
  RUN_USER=tg-support-bot
  chown -R "$RUN_USER:$RUN_USER" "$APP_DIR"
  chmod 600 "$APP_DIR/.env"
fi

if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then
  cat > /etc/systemd/system/tg-rust-support-bot.service <<UNIT
[Unit]
Description=Telegram Support & Feedback Bot
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$APP_DIR
EnvironmentFile=$APP_DIR/.env
ExecStart=$APP_DIR/bot
Restart=on-failure
RestartSec=5
User=$RUN_USER

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable --now tg-rust-support-bot.service
  echo "Installed and started. Logs: journalctl -u tg-rust-support-bot -f"
else
  echo "Installed at $APP_DIR/bot; systemd is unavailable. Start with: $APP_DIR/bot"
fi
