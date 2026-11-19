# Каталог DEX BSC — живые данные 19.11.2026 (решение 434)

Источник: архив сервера бота «dex-catalog-20261119» (sha256 архива здесь не записан — в репо лежат распакованные
файлы; sha256 каждого исходного файла — ниже). Снято 19.11.2026 на сервере бота: вызовы узла BSC — 23:30:32–23:43:41
UTC (поле `t` в raw_rpc_node.json), все eth_call / eth_getCode — с тегом блока `latest` (номер блока не записан).
Только чтение. Скрипты сбора (gt.py, pools_probe.py, eth.py) в репо не кладутся — они не нужны для тестов и разбора.

| файл | что |
|---|---|
| `geckoterminal_dexes_pages.json` | все DEX сети BSC у GeckoTerminal: `GET api.geckoterminal.com/api/v2/networks/bsc/dexes?page=1..3` — 140 id |
| `defillama_protocols.json.xz` | список протоколов DefiLlama (9079; поля id, name, category, chains, chainTvls, …). Сжат xz; URL запроса в архиве не записан |
| `settings_dex_selected.json` | выгрузка живых настроек бота: dex.kind (84), dex.pool_enabled (90), dex.factory (81), dex.quoter (19), index.* (10) |
| `diff.json` | 67 id GeckoTerminal, у которых нет dex.kind |
| `pools_probe.json` | по каждому из 67: ответ GT `…/dexes/{id}/pools?page=1` и до 2 пулов: factory(), token0(), token1() пула и eth_getCode фабрики (в этом файле код обрезан — полный в raw_rpc_node.json) |
| `pools_probe.log` | журнал пробы: номер, id, статус GT, число пулов |
| `raw_rpc_node.json` | 363 сырых вызова узла бота (127.0.0.1:8545): 279 eth_call (селекторы c45a0155 factory, 0dfe1681 token0, d21220a7 token1), 84 eth_getCode; 17 ответов — откат |

Полные sha256:
```
b1b8bdd08d0d51445a6d5ca498787e4f5e750fcd8144ab73329207f452010e6e  defillama_protocols.json
ebceb3125138a85d49f55099b299b444b6579c5f5fd7bde30d26915178a378d6  diff.json
e7928b1266e809f11a0234ece0adcdac5a024d94b341bfdf677ece7e9184a536  geckoterminal_dexes_pages.json
e82a8e7fa06aeab34ae8f9dcae1ac473db6a8ead7bfe2e17f4477a1f516df36a  pools_probe.json
33f127d23971e41078dbb2d5c0889a18b9a647021b56d2bf339725ddd033c72d  pools_probe.log
bb8261151f674652506c4de7f0497b71225fde119658539c24165a0201226e20  raw_rpc_node.json
7a80f44b6be578f532f6198a802432d7990e62bd75f117eaecbe24544f97c363  settings_dex_selected.json
```

Не положено: `settings_all.json` (полная выгрузка настроек, 1818 строк) — в ней id чатов Telegram, IP-адреса сервера
для ключей бирж и адрес исполнителя кошелька; для разбора DEX нужна только `settings_dex_selected.json`.

Разбор — docs/notes/bsc-dex-coverage-2026-11-20.md (решение 434). Тестов на этих файлах пока нет.
