#!/bin/sh
# Файл: scripts/ledger_rollback.sh
# Что делает: решение 353 (ревью t158 Б3) — откат на версию бота ДО t158 (учёт ядра в arb.sqlite). Переносит учёт ядра
# (таблицы core_state, daily_result, wallet_queue) из ledger.sqlite обратно в arb.sqlite: строки ledger.sqlite —
# правда (новая версия писала только туда), заменяют одноимённые строки arb.sqlite одной транзакцией. Ничего не удаляет.
# Перед этим — копии обоих файлов рядом (имя + .before-rollback-<время>).
# Кто вызывает: человек на сервере, при ОСТАНОВЛЕННОМ боте, перед запуском старой версии.
# От чего зависит: sqlite3 (командная строка SQLite), sh.
# Аргументы: папка данных бота (--data-dir бота).
# Повторное обновление после отката — scripts/ledger_reimport.sh (без него новая версия не перенесёт то, что старая
# версия записала в arb.sqlite после отката: перенос делается один раз).
set -eu
dir="${1:?папка данных бота (как --data-dir)}"
main="$dir/arb.sqlite"
led="$dir/ledger.sqlite"
[ -f "$main" ] || { echo "нет $main" >&2; exit 1; }
[ -f "$led" ] || { echo "нет $led — откатывать нечего" >&2; exit 1; }
stamp=$(date -u +%Y%m%dT%H%M%SZ)
# Копия — через .backup (согласованный снимок вместе с WAL).
sqlite3 "$main" ".backup '$main.before-rollback-$stamp'"
sqlite3 "$led" ".backup '$led.before-rollback-$stamp'"
sqlite3 "$main" <<SQL
.bail on
ATTACH '$led' AS l;
BEGIN;
INSERT OR REPLACE INTO core_state (kind, id, body, updated_at) SELECT kind, id, body, updated_at FROM l.core_state;
INSERT OR REPLACE INTO daily_result (day, body, updated_at) SELECT day, body, updated_at FROM l.daily_result;
INSERT OR REPLACE INTO wallet_queue (venue, body, updated_at) SELECT venue, body, updated_at FROM l.wallet_queue;
COMMIT;
SQL
echo "Учёт ядра перенесён из $led в $main. Копии: *.before-rollback-$stamp. Можно запускать старую версию."
