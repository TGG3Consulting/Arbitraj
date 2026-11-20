#!/usr/bin/env python3
"""Файл: tools/arbitrum_record/zyberswap_v3.py
Что делает: решение 487 (T487-01, T487-02) — запись своего узла Arbitrum для Zyberswap V3 (вид thena_fusion: код пула
  и комиссии = THENA Fusion по Sourcify, docs/notes/arbitrum/dex-settings-arbitrum.md §4.7). Тот же ход и тот же формат
  файлов, что у записи THENA Fusion BSC (tools/bsc_record/thena_fusion.py + thena_fusion_run.py: состояние, сжатые и
  сырые слова tickTable, ticks(), слот 3, оракул комиссии — оператор, feeConfig, getFee / getAverages / getTimepoints
  на время блока + 0 / 1 / 3 с, начало окна oracle.starts, котировки виртуального квотера на 4 временах), с отличиями
  сети: eth_chainId = 42161 (проверка до чтения пулов); GeckoTerminal не спрашивается (пулов Zyberswap V3 там нет —
  dex49/1.md стр. 104) — пулы poolByPair фабрики по всем парам монет --pair-token и/или --pool; официального квотера
  нет (--quoter "" — только виртуальный квотер, обмен самого пула); heads.json — времена последних --heads блоков
  (T487-02: следующий блок в ту же или следующую секунду?). Только чтение.
Адреса (источники — docs/notes/arbitrum/dex-settings-arbitrum.md): фабрика 0x9c2abd632771b433e5e7507bcaa41ca3b25d8544
  (адрес из адаптера DefiLlama, код подтверждён Sourcify 42161; принято координатором по правилу Тиграна 20.11); WETH
  0x82aF49447D8a07e3bd95BD0d56f35241523fBab1 (Uniswap docs v3-arbitrum-deployments, «Wrapped Native Token»); USDC
  0xaf88d065e77c8cC2239327C5EDb3A432268e5831 (quoteToken() пула WooPP на записи своего узла, решение 462). Хеш кода
  пула Zyberswap (POOL_INIT_CODE_HASH фабрики, Sourcify 42161) — 0x6ec6…94a4, тот же, что AlgebraV1 по умолчанию.
Запуск (из корня репозитория): python3 tools/arbitrum_record/zyberswap_v3.py --rpc http://УЗЕЛ:8547 (или record_all.py,
  шаг zyberswap_v3). Папка: tests/data/arbitrum_zyberswap_v3_<дата UTC>/ и zip.
Кто вызывает: Тигран; record_all.py (record_steps.py). От чего зависит: tools/bsc_record/thena_fusion.py (args,
  pool_record), thena_fusion_run.py (run), rec_rpc.py, stdhttp.py.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "tools", "bsc_record"))
from thena_fusion import args  # noqa: E402
from thena_fusion_run import run  # noqa: E402

DEFAULTS = {
    "chain_id": 42161,
    "name": "arbitrum_zyberswap_v3",
    "gecko_network": "arbitrum",
    "gecko_dex": "",
    "gecko_pools": 0,
    "factory": "0x9c2abd632771b433e5e7507bcaa41ca3b25d8544",
    "quoter": "",
    "pair_token": ["0x82aF49447D8a07e3bd95BD0d56f35241523fBab1", "0xaf88d065e77c8cC2239327C5EDb3A432268e5831"],
    "heads": 200,
}


if __name__ == "__main__":
    o = args(DEFAULTS, __doc__)
    run(o)
