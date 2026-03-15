#!/bin/sh
set -eu

if [ "${SIP_RESOLVE_HOST:-true}" = "true" ] && [ -n "${SIP_HOST:-}" ]; then
  case "$SIP_HOST" in
    *[!0-9.]*)
      resolved_host="$(getent ahostsv4 "$SIP_HOST" | awk 'NR==1 { print $1 }')"
      if [ -n "$resolved_host" ]; then
        echo "Resolved SIP_HOST $SIP_HOST -> $resolved_host"
        export SIP_HOST="$resolved_host"
      else
        echo "Failed to resolve SIP_HOST=$SIP_HOST" >&2
        exit 1
      fi
      ;;
  esac
fi

if [ "$#" -eq 0 ] && [ -n "${AGENT_VOICE_CONFIG:-}" ]; then
  set -- --config "$AGENT_VOICE_CONFIG"
fi

exec agent_voice "$@"
