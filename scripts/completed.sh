#!/bin/bash
set -eu

PATH=/usr/bin
cfgpath=/etc/trctl/completed.toml

. /usr/lib/trctl/logging.sh

name=$TR_TORRENT_NAME
hsh=$TR_TORRENT_HASH

fipath=$(sed -rn 's/default_destination = "(.+)"/\1/p' "$cfgpath")
mailuser=$(sed -rn 's/mailuser = "(.+)"/\1/p' "$cfgpath")
if [ "$mailuser" = none ]; then
    mailuser=
fi

errok=0

checkstatus() {
  err=$?
  if [ $err != 0 ] && [ $errok = 0 ]; then
    log error "terminating on error: $err"
  fi
}

notify() {
  local subject msg
  subject="Torrent completed =$fipath="
  msg=$name
  if [ -z "${mailuser:-}" ]; then
    return 0
  fi

  # sendmail does not properly escape utf-8
  # does not work with no new privileges
  printf '%s\n' "$msg" | mail -S mime-encoding=8bit -n -s "$subject" "$mailuser"
  #log notice "$subject: $msg"
}

trap checkstatus 0

logged=0
while [ "$#" -gt 0 ]; do
  case "$1" in
    --systemd)
      export shlib_systemd=1
      ;;
    --logged)
      logged=1
      ;;
    *)
      error "unknown arg: $1"
      ;;
  esac
  shift
done

if [ "$logged" = 0 ]; then
  exec systemd-cat -p notice -t completed --level-prefix=yes sh "$0" "$@" --systemd --logged
fi
notify
trctl --config "$cfgpath" --yes mv --hsh "$hsh"
