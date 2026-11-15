# KyberSwap Classic DMM (kyberswap_classic_bsc) — сырые ответы узла BSC, 15.11.2026

Записано Claude 15.11.2026 из своей вкладки браузера (JSON-RPC к https://bsc-rpc.publicnode.com, только чтение и
eth_call с подменой кода — в сеть ничего не уходило; GeckoTerminal API v2 — список DEX и пулов; Sourcify v2 — исходники).

`probe.json` (блок 133682402):
- фабрика DMMFactory 0x878dfe971d44e9122048308301f540910bbd934c — Sourcify match (DMMFactory.sol, solc 0.6.12; DMMPool,
  VolumeTrendRecorder, FeeFomula, MathExt — в том же файле), PUSH4 getPools 0x5b1dc86f в коде, allPoolsLength = 133,
  getPools(USDT, WBNB) — 4 пула (у пары несколько пулов с разным усилением);
- роутер DMMRouter02 0x78df70615ffc8066cc0887917f2cd72092c86409 — Sourcify match, factory() = фабрика; адрес — официальная
  документация docs.kyberswap.com (Classic Contract Addresses → BSC → Dynamic Fee);
- четыре пула (factory() = фабрика, код 19 176 байт, PUSH4 getTradeInfo 0xd6694027 и swap 0x022c0d9f): ответ getTradeInfo
  [reserve0, reserve1, vReserve0, vReserve1, feeInPrecision], ampBps и ответы роутера getAmountsOut(amount, [пул],
  [tokenIn, tokenOut]) на 3 суммах (0,01 %, 1 %, 10 % реального резерва входа) в обе стороны — 24 ответа;
- id DEX у GeckoTerminal — `kyberswap_classic_bsc` (13 пулов; 2 из них — не из этой фабрики: пулы KSFactory со
  статической комиссией, другой вид — в этот вид не берутся, проверка их отклонит по factory()).

`exec_sim.json` — шаг KYBER_DMM (вид 15) ArbExecutor v2 через eth_call с подменой кода (код решения 287, sha256 байтов
354e3f81ab111372…): получено ровно getAmountsOut роутера во всех 5 случаях (3 пула, обе стороны); тот же маршрут видом V2
— откат.

`source_excerpt.txt` — дословные выдержки официального исходника KyberNetwork/dmm-smart-contracts@e557b57 с номерами строк:
события и swap пула, getTradeInfo, verifyBalanceAndUpdateEma (проверка K по виртуальным резервам с комиссией блока),
getFinalFee, DMMLibrary.getAmountOut / getAmountIn, MathExt.PRECISION, VolumeTrendRecorder (комиссия блока не зависит от
объёма самого обмена), PoolCreated / getPools / isPool фабрики, DMMRouter02.getAmountsOut. Сверка исходника с кодом BSC:
тексты MathExt, FeeFomula, VolumeTrendRecorder, DMMFactory из Sourcify совпали с репозиторием без пробелов; DMMPool —
кроме _mintFee (доля протокола при mint/burn; на обмен не влияет).

Используется: tests/bsc_dex_kyber_dmm_live.rs (решение 287).
