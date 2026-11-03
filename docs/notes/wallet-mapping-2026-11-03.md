# Маппинг кошельков DEX (решение 132) — план и ход работ

> Ветка `router/wallets` от `feat/own-bsc-node` (e6c390a). Решение Тиграна 03.11.2026: у каждой биржи свой кошелёк DEX,
> своё исполнение и учёт; DEX остаётся одной площадкой `bsc_dex` с общими пулами, котировками и настройками.

## Что сейчас использует кошелёк (до решения 132)

Один секрет `bsc_dex.wallet_key` и одна настройка `exec.executor_address [bsc_dex]` на все сделки.

| Где | Что берёт | Файлы |
| --- | --- | --- |
| Ключ и адрес | `wallet::signer::{signer,address,sign}` из секрета | venues/bsc_dex/wallet/signer.rs, ctx.rs (`SECRET_WALLET_KEY`) |
| Очередь и nonce | `Nonces` — одна на адаптер; состояние в SQLite `wallet_queue` по ключу `bsc_dex` | wallet/nonce.rs, wallet/queue_store.rs, storage/wallet_queue.rs, exec/txs.rs, send/cancel.rs, send/settle.rs |
| Разрешения (approve) и их кэш | `Allowances` — одна на адаптер | wallet/allow_cache.rs, wallet/approve.rs, wallet/approve_executor.rs, quote/approve_gas.rs, exec/* |
| Ожидаемые поступления | `Inflows` — одна на адаптер | wallet/inflows.rs, wallet/deposit.rs, venue.rs (deposit_address) |
| Балансы | `balances::{all,single}` по адресу ключа | wallet/balances.rs, wallet/balances_mc.rs |
| Переводы с кошелька | `transfer::withdraw`, статус, стоимость | wallet/transfer.rs, transfer_status.rs, transfer_cost.rs |
| События баланса (решение 76) | Transfer на/с адреса ключа | wallet/events*.rs (задача — resubscribe.rs) |
| Исполнитель ArbExecutor | адрес — `exec.executor_address`, владелец — кошелёк | exec/executor/{mod,onchain,deploy,trade,dry}.rs, pools/exec_kind.rs, quote/graph_exec.rs, quote/graph_executor.rs, check/probe_exec.rs, exec/cycle_gate.rs |
| Сделки DEX | подпись, очередь, исполнитель | exec/{trade,trade_direct,trade_smart,swap_send,bundle_*,cycle_*}.rs, best_of/trade.rs |
| Пробы налогов (симуляция от адреса) | адрес ключа | check/taxes.rs, check/service_tax.rs, check/probe_exec.rs |
| Сухие сделки, оценка циклов | адрес ключа | exec/dry_swap.rs, exec/cycle_dry.rs, quote/cycle_cost.rs |
| «Проверить подключение» DEX | адрес и баланс ключа | connection.rs |
| KyberSwap (exec.via_kyber) | тот же секрет, общая очередь (решение 112) | venues/kyberswap/{ctx,signer,tx,trade_prep,transfer,balances}.rs, venues/bsc_dex/mod.rs |
| Ядро — путь сделки | `gw.get(площадка)` без кошелька | core/{trade_ops,transfers,limits,precheck,precheck_deposit,costs_quote,costs_withdraw,profit,prices,relay_value,credit_expect,credit_mismatch,transfer_credit,manual_halted,cycle_plan,cycle_deal,cycle_watch,cycle_dry,reconcile,low_usdt}.rs |
| API | без кошелька | api/{dex_executor,wallet_queue,check,dex_dry}.rs |

## Как пойдёт id счёта

1. **Хранилище** `settings/accounts/` (SQLite, своя история «было/стало», журнал `wallets.changed`): кошельки
   {id, имя, имя секрета ключа, адрес исполнителя}, маппинг {строка → id кошелька}. Строки маппинга: id каждой CEX
   из реестра шлюза, `dex_dex` (циклы), `probe` (пробы и сигналы — симуляции без денег, см. ниже). Ключ — в
   зашифрованном файле секретов (`wallet.<id>.key`); в SQLite — только имя секрета. Доступ — `Store::accounts()`.
2. **Ядро** знает только `DexAccountId` (core/types/account.rs). Направление CEX ↔ DEX → строка «биржа»; DEX ↔ DEX →
   строка `dex_dex`; CEX ↔ CEX — без счёта. Нет строки — отказ с причиной «нет кошелька DEX в маппинге (экран
   «Маппинг кошельков»)». Сделка запоминает `dex_account` (Deal, JSON в учёте); все шаги сделки берут площадку через
   `core/dex_account.rs` по счёту сделки, не по текущему маппингу.
3. **Шлюз**: `Registry::get_as(площадка, счёт)` → для DEX `Venue::for_account(счёт)` — вид адаптера, привязанный к
   кошельку. CEX-адаптеры не меняются (по умолчанию счёт им не нужен).
4. **Адаптер bsc_dex**: `Ctx.account` (id счёта); `Parts::bound(id)` — своя очередь/nonce, разрешения, ожидаемые
   поступления на каждый кошелёк (общие пулы, котировки, настройки). Ключ и исполнитель — по записи кошелька.
   Непривязанный адаптер — только чтение: подписать или отправить транзакцию, дать адрес депозита, балансы — отказ
   «кошелёк DEX не выбран» (защита: потерянный счёт не уводит деньги на чужой кошелёк). Пробы налогов, оценки
   разрешений и сигналы без сделки — от кошелька строки `probe`.
5. **KyberSwap**: `kyberswap::Ctx.wallet` — имя секрета кошелька счёта; очередь — очередь этого кошелька (решение 112).
6. **Перенос** (один раз): нет ни одного кошелька → кошелёк «BingX» с секретом `bsc_dex.wallet_key` (ключ не
   переписывается), исполнитель — текущее значение `exec.executor_address` (если задано), строки `bingx` и `probe` →
   «BingX». Очередь кошелька «BingX» — прежняя запись `wallet_queue` (ключ `bsc_dex`). Сделки без счёта (старые) —
   кошелёк переноса. Деньги не двигаются.

## Этапы (каждый коммит зелёный)

1. План, хранилище кошельков и маппинга, перенос, тип ядра, тесты хранилища.
2. Адаптер: привязка к счёту, свои состояния кошелька, ключ и исполнитель по счёту, KyberSwap, запрет записи без счёта.
3. Ядро: счёт направления и сделки, причины, все шаги сделки и ручные действия через счёт, сверка балансов и «мало
   USDT/BNB» по кошелькам.
4. API, экран «Маппинг кошельков», уведомления, документы (SECURITY, UI, SETTINGS, ARCHITECTURE, DECISIONS, TASKS).

## Ход работ

- Этап 1 — хранилище `settings/accounts/`, перенос, `DexAccountId`, тесты `tests/wallet_accounts.rs`.
- Этап 2 — адаптер: `Ctx.account` + `ctx_wallet.rs` (кошелёк счёта / «пробы», `bound()` — счёт обязателен для подписи,
  очереди, адреса депозита, балансов, сделки, вывода, исполнителя), `parts_wallet.rs` (очередь, разрешения, поступления
  по кошелькам), `venue_account.rs` (`for_account`), KyberSwap — `kyberswap::Ctx.wallet` и очередь кошелька,
  `resubscribe_wallets.rs` (очередь и события баланса каждого кошелька), `WalletQueue.account`, `BalanceChange.account`
  (у бирж — None), `exec.executor_address` убрана из каталога (исполнитель — у кошелька). Шлюз: `Registry::get_as`.
  Тесты: `tests/bsc_dex_accounts.rs`. После этапа 2 ядро ещё не передаёт счёт — сделки с DEX отказывают «кошелёк не
  выбран» до этапа 3 (ветка не слита).
