# venues/bsc_dex/wallet — кошелёк бота

Один кошелёк (решение 45). Ключ — секрет `bsc_dex.wallet_key`, только в памяти, в журнал и ошибки не попадает.

| Файл | Что делает |
| --- | --- |
| `mod.rs` | Модуль |
| `signer.rs` | Ключ → подписант alloy, адрес, подпись EIP-155/EIP-2718, хеш до отправки (тест — официальный пример EIP-155) |
| `nonce.rs` | Строгая очередь: одна транзакция кошелька за раз; nonce — `eth_getTransactionCount(адрес, "latest")` |
| `balances.rs` | Балансы: BNB (`eth_getBalance`) и известные токены (`balanceOf`) |
| `approve.rs` | Разрешения ERC-20 → Permit2 и Permit2 → Universal Router: каких не хватает (`dex.approve_unlimited`, `dex.permit2_expiration_s`) |
| `transfer.rs` | Перевод монеты из кошелька на адрес (вывод на биржу), стоимость перевода |
| `transfer_status.rs` | Статусы вывода и зачисления по квитанции |
