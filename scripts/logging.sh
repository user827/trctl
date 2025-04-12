# allows to see errors with -e even when this function is not used
geterr() {
  _err=0
  "$@" || _err=$?
}

log_sd() {
  local lvl="$1"
  shift
  case "$lvl" in
    fatal)
      printf "<2>%s\n" "$*" >&2
      ;;
    err|error)
      printf "<3>%s\n" "$*" >&2
      ;;
    warn|warning)
      printf "<4>%s\n" "$*" >&2
      ;;
    notice)
      printf "<5>%s\n" "$*"
      ;;
    info)
      printf "<6>%s\n" "$*"
      ;;
    debug)
      printf "<7>%s\n" "$*"
      ;;
    *)
      printf "<3>%s\n" "log: unknown log level: $lvl" >&2
      return 1
      ;;
  esac
}

log() {
  if [ -n "${shlib_systemd:-}" ]; then
    geterr log_sd "$@"
    return $_err
  fi

  if [ ! -t 1 ]; then
    geterr log_basic "$@"
    return $_err
  fi

  local lvl="$1"
  shift
  case "$lvl" in
    fatal|error)
      printf '\033[31m%s\033[0m\n' "$*" >&2
      ;;
    warn)
      printf '\033[33m%s\033[0m\n' "$*" >&2
      ;;
    notice)
      printf '\033[32m%s\033[0m\n' "$*"
      ;;
    info|debug)
      printf '%s\n' "$*"
      ;;
    *)
      printf '\033[31m%s\033[0m\n' "log: unknown log level: $lvl" >&2
      return 1
      ;;
  esac
}

log_basic() {
  local lvl="$1"
  shift
  case "$lvl" in
    fatal|error|warn)
      printf '%s\n' "$*" >&2
      ;;
    notice|info|debug)
      printf '%s\n' "$*"
      ;;
    *)
      printf '%s\n' "log: unknown log level: $lvl" >&2
      return 1
      ;;
  esac
}
