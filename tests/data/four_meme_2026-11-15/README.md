# four.meme (BSC) — сырые ответы узла BSC и выдержки, 15.11.2026 (решение 291: не подключается)

Записано Claude 15.11.2026 из своей вкладки браузера (JSON-RPC к https://bsc-rpc.publicnode.com — только eth_call и
eth_getLogs, в том числе eth_call от адреса держателя без подмены состояния; в сеть ничего не уходило; GeckoTerminal API v2;
Sourcify v2; страница BscScan реализации TokenManager2; официальная документация four.meme — four-meme.gitbook.io →
github.com/four-meme-community/fourmeme-docs@5f7f589b). Совпадение `probe.json` с ответом вкладки проверено SHA-256
строки JSON (4e89daae…b5a761c).

`probe.json` (блок 133716094, время блока 1794736897):
- TokenManager2 0x5c95…762b — прокси ERC1967 (Sourcify match: ERC1967Proxy); реализация по слоту EIP-1967 —
  0xef1e8d3fe13ab8497516b5a117969bd2a86d6cff (24 362 байта): Sourcify 404; BscScan — «Are you the contract creator?
  Verify and Publish your contract source code today!» (исходник не опубликован);
- TokenManagerHelper3 0xF251…6034 — прокси ERC1967 (Sourcify match), реализация 0x0cc78251…43c8 — Sourcify 404;
- два токена на кривой (из событий TokenPurchase TokenManager2): 0x719f…4444 (котируемая — USDT; клон реализации
  0x4686…a986 — Sourcify match, Token3.sol) и 0xa7bc…ffff (котируемая — BNB, quote = 0; клон 0x7330…5c94 — Sourcify match,
  Token9.sol): getTokenInfo Helper3 (version 2, liquidityAdded 0, tradingFeeRate 100 = 1 %), _mode = 1
  (MODE_TRANSFER_RESTRICTED), eth_call transfer(всего баланса) от держателя другому кошельку и transfer 1e9 на адрес контракта
  WBNB — оба true; tryBuy (budget) и trySell — ответы Helper3;
- `gt_four_meme_page1` — GeckoTerminal id DEX `four-meme`, страница 1 (20 «пулов»): адрес «пула» = адрес самого токена;
  у всех 20 getTokenInfo.liquidityAdded = 0 и нет пула PancakeSwap V2 (getPair с WBNB, USDT) и V3 (getPool с WBNB, USDT,
  комиссии 100 / 500 / 2500 / 10000);
- `liquidity_added_search` — событий LiquidityAdded TokenManager2 в блоках 133596095–133716094 не найдено (0).

`source_excerpt.txt` — официальная документация (README, integration-guide, trade-guide, ITokenManager2.sol) с номерами
строк и исходник токена (FourToken.sol, Token3.sol, Token9.sol — Sourcify) с номерами строк.

Используется: docs/DECISIONS.md (решение 291) — не подключается; тестов нет (кода нет).
