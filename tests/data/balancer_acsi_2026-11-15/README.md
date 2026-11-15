# Balancer V2 / ACSI Finance (acsi-finance) — сырые ответы узла BSC, 15.11.2026

Записано Claude 15.11.2026 из своей вкладки браузера (JSON-RPC к https://bsc-rpc.publicnode.com, только чтение и
eth_call, в том числе с подменой кода и балансов — в сеть ничего не уходило; GeckoTerminal API v2 — список DEX и пулов;
Sourcify v2 — исходники и сведения о развёртывании; GitHub balancer/balancer-deployments@b55dc2b8 — build-info).
Ответы переписаны из страницы вкладки; совпадение переписанного с ответом вкладки проверено SHA-256 строки JSON
(probe — 7cada740…36db1f, exec — a27195dc…, GIVEN_OUT — 23091929…188d5d).

`probe.json` (блок 133699740, время блока 1794729535):
- хранилище Vault 0xa82f327bbbf0667356d2935c6532d164b06ceced — Sourcify exact_match (contracts/vault/Vault.sol, solc 0.7.1,
  optimizer 1500, блок 7682300); 45 из 45 исходных файлов = balancer-deployments v2/tasks/20210418-vault/build-info/
  Vault.json (тот же код, что у Vault Balancer). WETH() = WBNB, пауза хранилища — нет (окна закончились в 2021),
  ProtocolFeesCollector 0x86182be6… — getSwapFeePercentage = 0, getFlashLoanFeePercentage = 1e14 (0,01 %; обмены не трогает);
- фабрики: WeightedPoolFactory 0xb7238020… (Sourcify exact_match, 36 из 36 = 20210418-weighted-pool) и StablePoolFactory
  0xd2eada8b… (exact_match, 40 из 40 = 20210624-stable-pool), getVault() обеих = хранилище; транзакции создания пулов
  0x7ea9f435 / 0x6e5dbc9a / 0xa22f3e51 / 0xe901c4e9 адресованы этим фабрикам;
- GeckoTerminal: id DEX `acsi-finance`, 24 пула (страницы 1–2), ликвидность от 2 до 6 702 USD (сумма ≈ 24,0 тыс. USD);
  21 пул — из фабрик (18 WeightedPool: код 18 286 байт = deployedBytecode WeightedPool build-info при обнулённых
  immutableReferences; 3 StablePool: 18 260 байт = StablePool), 3 пула (0xb978bd52, 0x86ddc49f, 0xdaae64e8; 19 408 байт)
  не из фабрик: isPoolFromFactory = 0 у обеих, код не равен WeightedPool2Tokens 20210418 (19 384 байта), Sourcify 404 —
  исходник НЕ установлен, видом не берутся;
- девять пулов: getVault, getPoolId (pool_id), getPool(id) хранилища, getPoolTokens(id), getSwapFeePercentage,
  getPausedState, getNormalizedWeights / getAmplificationParameter, decimals токенов, isPoolFromFactory обеих фабрик;
  `queries` — queryBatchSwap GIVEN_IN на 0,01 %, 1 %, 10 % баланса входа (пары и обе стороны) и 35 % (откат BAL#304 —
  MAX_IN_RATIO); у StablePool 0xe901c4e9 (TUSD/UST/USDT/BUSD/MIM, A = 16) все запросы откатываются BAL#322
  (STABLE_GET_BALANCE_DIDNT_CONVERGE); `queries_out` — GIVEN_OUT (точный выход) на тех же долях баланса выхода.

`exec_sim.json` — шаг BALANCER (вид 17) ArbExecutor v2 решения 289 через eth_call с подменой кода (contracts/out/
ArbExecutor.bin-runtime, 15 946 байт; совпадение кода проверено SHA-256 байтов в вкладке): получено ровно столько, сколько
отвечает queryBatchSwap на том же блоке, — 9 одиночных шагов (весовые 3 и 4 токена, весовые 2 токена TWO_TOKEN,
стейблсвапы GENERAL, обе стороны) и маршрут USDT → BUSD (стейбл) → WBNB (весовой); шаг с poolId чужого пула — откат
BAL#521. Тест сверяет received с НАШЕЙ формулой (math/balancer_math.rs) на состоянии probe.json того же блока.

`source_excerpt.txt` — дословные выдержки build-info с номерами строк: IVault (getPool, getPoolTokens, swap, SingleSwap,
BatchSwapStep, FundManagement, queryBatchSwap, события), PoolRegistry (registerPool, getPool, раскладка id), Swaps (swap,
_swapWithPool, полные балансы в onSwap, queryBatchSwap), AssetTransfersHandler (_receiveAsset), FixedPoint, LogExpMath,
WeightedMath, BaseMinimalSwapInfoPool, BasePool (обе версии: комиссия, множители), WeightedPool, TemporarilyPausable,
BasePoolFactory, StableMath, BaseGeneralPool, StablePool (A и рампа), Math.

Используется: tests/bsc_dex_balancer_acsi_live.rs (решение 289).
