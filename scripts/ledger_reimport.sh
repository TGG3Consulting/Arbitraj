#!/bin/sh
# Файл: scripts/ledger_reimport.sh
# Что делает: решение 353 (ревью t158 Б3) — повторный перенос учёта ядра после отката: новая версия переносит учёт из
# arb.sqlite в ledger.sqlite ОДИН раз (отметка в таблице meta файла учёта), поэтому после отката (scripts/
# ledger_rollback.sh) и работы старой версии свежий учёт лежит в arb.sqlite, а ledger.sqlite устарел. Скрипт убирает
# ledger.sqlite (и его -wal, -shm) в сторону (имя + .before-reimport-<время>); следующий запуск новой версии создаст
# файл учёта заново и перенесёт в него учёт из arb.sqlite (строка журнала storage.ledger).
# Кто вызывает: человек на сервере, при ОСТАНОВЛЕННОМ боте, перед запуском новой версии после отката.
# От чего зависит: sqlite3, sh.
# Аргументы: папка данных бота (--data-dir бота).
set -eu
dir="${1:?папка данных бота (как --data-dir)}"
led="$dir/ledger.sqlite"
[ -f "$led" ] || { echo "нет $led — следующий запуск перенесёт учёт сам" >&2; exit 0; }
stamp=$(date -u +%Y%m%dT%H%M%SZ)
sqlite3 "$led" ".backup '$led.before-reimport-$stamp'"
for f in "$led" "$led-wal" "$led-shm"; do
  if [ -f "$f" ]; then mv "$f" "$f.moved-$stamp"; fi
done
echo "ledger.sqlite убран в сторону (копия: $led.before-reimport-$stamp). Следующий запуск перенесёт учёт из arb.sqlite."
