@echo off
rem ARB start. Data folder: data (next to this file). UI: http://127.0.0.1:8787 (this PC only).
cd /d "%~dp0"
arb.exe --data-dir data --listen 127.0.0.1:8787 --live-buffer 1000 --ui-dir ui
pause
