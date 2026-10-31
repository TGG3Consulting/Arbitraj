#!/usr/bin/env bash
# ARB start (Linux). Папка данных — data рядом с этим файлом. Интерфейс — http://127.0.0.1:8787 (только этот сервер).
# Аргументы запуска — src/cli.rs; порт и размер буфера живых потоков можно поменять прямо здесь.
# На сервере интерфейс наружу не открывать — заходить через SSH-туннель или VPN.
set -e
cd "$(dirname "$0")"
chmod +x ./arb 2>/dev/null || true
exec ./arb --data-dir data --listen 127.0.0.1:8787 --live-buffer 1000 --ui-dir ui
