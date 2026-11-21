# tests/data/bingx_2026-11-21 — живые ответы REST BingX 2026-11-21

Куски ответов из сырого журнала бота на сервере (storage/raw_log.rs, поле `m` записей), без изменений внутри объектов —
отобраны только нужные тесту монеты и пары (обёртка ответа — как прислал BingX):

- `config_getall.json` — c = «rest /openApi/wallets/v1/capital/config/getall», t = 1795261756781
  (2026-11-21 11:49:16.781 UTC), источник raw/bingx/1795260782660.jsonl.zst; оригинал 862372 байт, sha256
  06027fcacd32956f4ca8a07ae836487ff2e62b7653c222e741f3aea1a00bb4e6 (1897 монет). Отбор (71 монета): все 51 монета с
  адресом контракта в сети ARBITRUM (у 50 адрес не совпадает ни с одним адресом BEP20 — USDT Arbitrum 0xfd086bc7…cbb9
  и 49 монет: VELA, AIDOGE, LAVA, SQD, MAGIC, GMX … — у многих ARBITRUM единственная сеть; у ZRO адрес ARBITRUM =
  адрес BEP20) и первые по алфавиту 20 монет с адресом BEP20 и парой «-USDT» status 1 без сети ARBITRUM (0G … ARTX).
- `symbols.json` — c = «rest /openApi/spot/v1/common/symbols», t = 1795259800908 (2026-11-21 11:16:40.908 UTC),
  источник raw/bingx/1795257182656.jsonl.zst; оригинал 680626 байт, sha256
  dde2ce065859d5f192bcdef2920e3035e7bd26bfa1444cfb84691d29e88de347 (2304 пары). Отбор: пары «<монета>-USDT» монет
  config_getall.json выше (69 пар, у USDT и SYK пары нет).

Время ответа symbols (t) тесты берут как время получения справочника пар (tests/t489_common symbols_at_ms).

Кто читает: tests/core_net_chain_489.rs, tests/core_net_chain_489_held.rs, tests/core_net_chain_489_bsc.rs.
