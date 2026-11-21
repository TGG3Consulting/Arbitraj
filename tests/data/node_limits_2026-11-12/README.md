# node_limits_2026-11-12

Пределы своего узла BSC (geth-bsc, полный, не архивный), записаны координатором на сервере 12.11.2026 (только чтение):
eth_getLogs — диапазон не больше 5000 блоков (toBlock − fromBlock ≤ 5000), больше — отказ −32602 «exceed maximum block
range 5000»; состояние на номер блока — только последние 128 блоков (H … H−127), старше — −32000 «historical state … is
not available». Используется: tests/bsc_dex_quoter_261.rs (разбор ответов), check/quoter_block.rs (шапка).
