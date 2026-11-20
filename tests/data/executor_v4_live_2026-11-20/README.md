# executor_v4_live_2026-11-20 — живая проверка ArbExecutor v4 на Arbitrum One (eth_simulateV1, свой узел)

Только чтение: eth_call / eth_getLogs / eth_getCode / eth_getBalance / eth_simulateV1. Ничего не подписано и не отправлено, ключи не использовались. Скрипт — tools/arbitrum_record/exec_v4_live.py (решения 400, 459).

## Параметры
- Код: contracts/out/ArbExecutor.bin-runtime (сборка v4, решение 457)
- chainId: 42161 (Arbitrum One; запись судит кошельки только этой сети — TASKS T457-02)
- sha256 симулированного bin-runtime: 2bf86e58e5719f88d709bcf06350570cbd60318f1e49e9f9d3a9c85c3ee10762 (совпал со строкой ArbExecutor.bin-runtime в ArbExecutor.sha256)
- H (блок симуляции и всех eth_call) = 526015524, время блока H = 1795201093, deadline = H.time + 600 = 1795201693; validation: false
- X (пустой адрес, на H: код 0x, баланс 0) = 0xc229fddb2d54db2eead21fa50e9611faab114c6c
- W (кошелёк, --wallet или пустой адрес) = 0x5d529b84907de6371ac3b0018d5cfe9eea233737
- Подмена — X: code + слот 0 = W, слот 1 = wnative 0x82aF49447D8a07e3bd95BD0d56f35241523fBab1; токены: баланс W = текущий + сумма входа, allowance(W, X) = 2^256−1 (слоты — slots.json)
- Выбор пулов — до симуляции, по квотеру на H (сумма — наибольшая из 10^(d+3)…10^(d−6) с выходом ≥ (1 − 3.0/100)·10·выход(сумма/10)); не больше 3 пулов на вид; пропуски — pools.json skipped
- x_left: ключ «usdt» — токен входа (а) (имя формата записи 17.11, live_row.rs), «coin» — второй токен

## Адреса
- Uniswap V4: PoolManager 0x360e68faccca8ca495c1b759fd9eee466db9fb32, V4Quoter 0x8928074CA1b241D8Ec02815881c1Af11E8bC5219, StateView 0x76fd297e2d437cd7f76d50f01afe6160f86e9990 (Initialize за 20000000 блоков до начала поиска; блок H взят после поиска — решение 460; без крючка, обе валюты ERC-20)
- Balancer V3: Vault 0xbA1333333333a1BA1108E8412f11850A5C319bA9, Router 0xEAedc32a51c510d35ebC11088fD5fF2b47aACF2E (пулы — запись balancer_v3.py + --bv3-pool; hooks строки = 0 — у шага 23 класс только «без крючка», крючок пула — bv3_hook, вид — bv3_hook_kind: none / static / surge / mev, как pools/balancer_v3_hooks.rs; без крючка до 3, с крючком до 2 на вид; нет пула вида: static, surge, mev)
- Fluid: резолвер 0x05Bd8269A20C472b148246De20E6852091BF16Ff; LB v2.0: фабрика 0x1886D09C9Ade0c5DB822D85D21678Db67B6c2982, роутер 0x7BFd7192E76D950832c77BB412aaE841049D8D9B
- Uniswap V3: фабрика 0x1F98431c8aD98523631AE4a59f267346ea31F984, QuoterV2 0x61fFE014bA17989E743c5F6cB21bF9697530B21e, уровни [500, 3000, 10000]; Uniswap V2: фабрика 0xf1D7CC64Fb4452F05c498126312eBE29f30Fbcf9, комиссия 3000 pips

## Счёт по видам (вердикт по классам — правило бота live_rule.rs; виды 0, 1, 22, 24 правило не судит)
- Вид 19, Uniswap V4: пулов 3; вызовов (а)/(б) 6, прошли 6, получено = квотер в том же состоянии 6, получено = кредит Swap 6, на исполнителе 0 — 6, остаток не записан (balanceOf(X) не ответил, x_left_error) — 0
- Вид 23, Balancer V3: пулов 1; вызовов (а)/(б) 2, прошли 2, получено = квотер в том же состоянии 1, получено = кредит Swap 2, на исполнителе 0 — 2, остаток не записан (balanceOf(X) не ответил, x_left_error) — 0
  - крючок none: пулов 1; вызовов (а)/(б) 2, прошли 2, получено = квотер в том же состоянии 1, получено = кредит Swap 2, на исполнителе 0 — 2, остаток не записан (balanceOf(X) не ответил, x_left_error) — 0
  - крючок static: нет пула
  - крючок surge: нет пула
  - крючок mev: нет пула
- Вид 22, Fluid DEX T1: пулов 3; вызовов (а)/(б) 6, прошли 6, получено = квотер в том же состоянии 6, получено = кредит Swap 0, на исполнителе 0 — 6, остаток не записан (balanceOf(X) не ответил, x_left_error) — 0
- Вид 24, LB v2.0: пулов 3; вызовов (а)/(б) 6, прошли 6, получено = квотер в том же состоянии 6, получено = кредит Swap 0, на исполнителе 0 — 6, остаток не записан (balanceOf(X) не ответил, x_left_error) — 0
- Вид 1, Uniswap V3: пулов 3; вызовов (а)/(б) 6, прошли 6, получено = квотер в том же состоянии 6, получено = кредит Swap 0, на исполнителе 0 — 6, остаток не записан (balanceOf(X) не ответил, x_left_error) — 0
- Вид 0, Uniswap V2: пулов 1; вызовов (а)/(б) 2, прошли 2, получено = квотер в том же состоянии 2, получено = кредит Swap 0, на исполнителе 0 — 2, остаток не записан (balanceOf(X) не ответил, x_left_error) — 0

## Файлы
- summary.json — по вызову: case, kind, pool_id, manager, pool_manager, hooks, token_in, token_out, amount_in, got, quoter_out, quoter_out_after_a (у (б)), got_eq_swap_credit, x_left, status, revert, gas
- calls.json — по вызову: status, returnData, gasUsed, логи целиком, откат, x_left
- hooks_reject.json — откаты крючков V4 (пулы V4 с крючком не выбираются — пусто; откат пула Balancer V3 с крючком — необъяснённое: класс 23 у бота один)
- slots.json, pools.json (выбранные, пропущенные, ошибки симуляции), raw_rpc.json — все запросы

## SHA-256 файлов
5ea7ed41ade3ba9cc4008d3a031f503b8ea29e507a6318deb7d22f2e50dbd062  calls.json
4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945  hooks_reject.json
b13fcf67e493687c55aa39172252667b88cb74f6fea9d995ab1c4c4343126337  pools.json
17f60b70c942be6760cbe6cded6754c89ca578d9b0661fbda1121f9b49a5ab1c  raw_rpc.json
0716086bc441b7dd0ea96ddefc610b040004219c83c0633bb87047419e09b4a2  slots.json
796dc4a005281d71c30ac5d7cc797b910a0a7fa9e55b9029de5c687fdec80c1e  summary.json
