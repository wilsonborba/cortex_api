#!/usr/bin/env bash
set -euo pipefail
case "${1:-status}" in
  start|stop|restart) exec systemctl --no-ask-password "$1" cortex-api.service ;;
  status) exec systemctl status cortex-api.service --no-pager ;;
  logs) exec journalctl -u cortex-api.service -n 50 -f ;;
  *) printf 'Use service.sh start|stop|restart|status|logs\n' >&2; exit 1 ;;
esac
