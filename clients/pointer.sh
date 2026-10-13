#!/bin/bash
# Drive `pointer serve --lan` from another machine (e.g. an agent in a container).
#   POINTER_HOST=http://192.168.1.10:8790  POINTER_TOKEN=<contents of token.txt next to pointer.exe>
# Usage mirrors the Windows CLI:
#   pointer.sh at "name" ["label"] | find "name" | shot [file.png] | point X Y ["label"]
#   pointer.sh wait [seconds] | busy ["msg"] | clip "text" | off | health
HOST=${POINTER_HOST:?set POINTER_HOST, e.g. http://192.168.1.10:8790}
TOKEN=${POINTER_TOKEN:?set POINTER_TOKEN}
CURL=(curl -sS --fail-with-body --connect-timeout 5 -H "Authorization: Bearer $TOKEN")
js() { python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$1"; }

case "$1" in
  at)     "${CURL[@]}" -X POST -d "{\"q\":$(js "$2"),\"label\":$(js "${3:-}")}" "$HOST/at" ;;
  find)   "${CURL[@]}" -G --data-urlencode "q=$2" "$HOST/find" ;;
  shot)   f=${2:-shot.png}
          "${CURL[@]}" "$HOST/shot" -o "$f" -D "$f.headers" || exit 1
          tr -d '\r' < "$f.headers" | awk -F': ' -v f="$f" '
            tolower($1)=="x-shot" {s=$2} tolower($1)=="x-cursor" {c=$2}
            tolower($1)=="x-cursor-monitor" {m=$2} tolower($1)=="x-monitor-rect" {r=$2}
            END {print f " " s; print "mouse at " c " on the " m " monitor (shot rect " r ")"}'
          rm -f "$f.headers"; exit 0 ;;
  point)  "${CURL[@]}" -X POST -d "{\"x\":$2,\"y\":$3,\"label\":$(js "${4:-}")}" "$HOST/point" ;;
  wait)   "${CURL[@]}" --max-time 960 "$HOST/wait?timeout=${2:-600}" ;;
  busy)   "${CURL[@]}" -X POST -d "{\"msg\":$(js "${2:-}")}" "$HOST/busy" ;;
  clip)   "${CURL[@]}" -X POST -d "{\"text\":$(js "$2")}" "$HOST/clip" ;;
  off)    "${CURL[@]}" -X POST "$HOST/off" ;;
  health) "${CURL[@]}" "$HOST/health" ;;
  *)      sed -n 2,6p "$0"; exit 2 ;;
esac
echo
