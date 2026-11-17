#!/usr/bin/env bash
# ЭКСПЕРИМЕНТ t175 — запуски теста-замера стека (exp-stack.yml). Бинарь теста — $BIN (собран шагом сборки).
# grid "<RETRIES…>" "<STACK_KIB…>" — по строке таблицы на каждую пару.
# threshold "<RETRIES…>" — наименьший STACK_KIB, на котором OK (деление пополам, шаг 8 КиБ, от 64 до 8192).
# Каждый запуск — отдельный процесс с || true: переполнение стека не валит job.
set -u

# Один запуск. Печатает OK / OVERFLOW / FAIL; вывод — в run.log.
one() {
  STACK_KIB="$2" RETRIES="$1" timeout 600 "$BIN" --ignored --exact chain_on_worker --nocapture >run.log 2>&1 || true
  if grep -q "^OK stack=" run.log; then
    echo OK
  elif grep -q "overflowed its stack" run.log; then
    echo OVERFLOW
  else
    echo FAIL
  fi
}

grid() {
  echo "| RETRIES | STACK_KIB | итог | подробности |"
  echo "|---|---|---|---|"
  for r in $1; do
    for s in $2; do
      res=$(one "$r" "$s")
      case "$res" in
        OK) more=$(grep -m1 "^OK stack=" run.log) ;;
        OVERFLOW) more=$(grep -m1 "overflowed its stack" run.log) ;;
        *) more=$(tail -n 5 run.log | tr '\n' ' ') ;;
      esac
      echo "| $r | $s | $res | $more |"
    done
  done
}

threshold() {
  echo "| RETRIES | порог, КиБ (наименьший OK) | наибольший OVERFLOW, КиБ | заметка |"
  echo "|---|---|---|---|"
  for r in $1; do
    lo=64
    hi=8192
    note=""
    if [ "$(one "$r" "$hi")" != OK ]; then
      echo "| $r | > $hi | — | на $hi не OK: $(tail -n 3 run.log | tr '\n' ' ') |"
      continue
    fi
    while [ $((hi - lo)) -gt 8 ]; do
      mid=$(((lo + hi) / 2))
      res=$(one "$r" "$mid")
      if [ "$res" = OK ]; then
        hi=$mid
      elif [ "$res" = OVERFLOW ]; then
        lo=$mid
      else
        note="FAIL на $mid: $(tail -n 3 run.log | tr '\n' ' ')"
        break
      fi
    done
    echo "| $r | $hi | $lo | $note |"
  done
}

"$@"
