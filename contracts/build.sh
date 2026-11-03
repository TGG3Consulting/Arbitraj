#!/usr/bin/env bash
# Файл: contracts/build.sh
# Что делает: сборка контракта-исполнителя ArbExecutor (решение 122, часть X): ставит solc-js ровно той версии, что в
#   package-lock.json (npm ci), собирает build.mjs и пишет артефакты в contracts/out/ (коммитятся в репозиторий вместе
#   с хешами ArbExecutor.sha256). Проверка, что артефакты в репозитории собраны из этих исходников: запустить и
#   убедиться, что `git diff contracts/out` пуст. Байткод для развёртывания бот берёт только из contracts/out/ и сверяет
#   его хеш с ArbExecutor.sha256 (src/venues/bsc_dex/exec/executor/code.rs).
# Кто вызывает: человек или агент после правки ArbExecutor.sol / IPools.sol.
# От чего зависит: node + npm, contracts/package.json, contracts/package-lock.json, contracts/build.mjs.
# Настройки: нет.
set -euo pipefail
cd "$(dirname "$0")"
npm ci --no-audit --no-fund
node build.mjs
