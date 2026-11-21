#!/usr/bin/env python3
"""Запись ответов своего узла Arbitrum One для crypto-пулов Curve (решения 442, 453).

Что делает: на ОДНОМ блоке (номер берётся у узла в начале) читает:
  * фабрики Twocrypto-NG и Tricrypto-NG — список пулов (pool_count, pool_list; пулы берутся С КОНЦА списка — новые);
  * у каждого пула — ВСЕ вызовы, которые делает бот (pools/load_crypto.rs calls(); для v1 — pools/load_crypto_v1.rs
    calls()), и проверочные (get_coins фабрики, version(), MATH()); ответ записывается по данным вызова (calldata) —
    тест берёт ответ на тот же calldata, что строит бот;
  * лесенку get_dy(i, j, dx) по каждой паре монет в обе стороны;
  * eth_getCode: реализаций пула (amm, amm_v2_0_0 Twocrypto; amm_native_transfers_enabled/disabled Tricrypto),
    математики (math, math_v2_0_0; math Tricrypto) и каждого пула — для сверки версии исходника по хешу кода;
  * фабрику crypto v1 (pool_count, pool_list, get_coins, get_decimals, код пула — прокси EIP-1167 — и код шаблона) и
    tricrypto 2021 (пул из журнала развёртывания) — решение 453; decimals() монет (точность v1 — по decimals).
Пулы Twocrypto-NG делятся по MATH(): файл twocrypto.json содержит и v2.1.0, и v2.0.0 (--max-pools на каждую
математику, если хватает пулов). Пишет JSON-файлы и README (блок, время блока, SHA-256 файлов) в
tests/data/arbitrum_curve_crypto_<дата>/. Ключей, подписей и транзакций нет — только eth_call, eth_getCode и
eth_getBlockByNumber (чтение).
Разбито по ревью 453 (файлы ≤ ~200 строк): curve_crypto_rpc.py — keccak256, ABI, JSON-RPC; curve_crypto_pools.py —
чтение пулов; здесь — аргументы, файлы и README записи. Модули лежат рядом и импортируются из папки скрипта.

Запуск (Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459); адреса — официальные журналы развёртывания curvefi, см. docs/FIRST_RUN.md):
    python3 tools/arbitrum_record/curve_crypto.py --node http://АДРЕС_УЗЛА:8547 \\
        --twocrypto-factory 0x98EE851a00abeE0d95D08cF4CA2BdCE32aeaAF7F \\
        --twocrypto-math 0x1Fd8Af16DC4BEBd950521308D55d0543b6cDF4A1 \\
        --twocrypto-math-v200 0x2005995a71243be9FB995DaB4742327dc76564Df \\
        --twocrypto-impls 0x934791f7F391727db92BFF94cd789c4623d14c52,0x04Fd6beC7D45EFA99a27D29FB94b55c56dD07223 \\
        --tricrypto-factory 0xbC0797015fcFc47d9C1856639CaE50D0e69FbEE8 \\
        --tricrypto-math 0x604388Bb1159AFd21eB5191cE22b4DeCdEE2Ae22 \\
        --tricrypto-impls 0xd7E72f3615aa65b92A4DBdC211E296a35512988B,0x1f7C86AffE5bCF7a1D74a8c8E2ef9E03BF31c1BD \\
        --v1-factory 0xE5De15A9C9bBedb4F5EC13B131E61245f2983A69 \\
        --v1-template 0x5bcA7dDF1bcccB2eE8e46c56bfc9d3CDC77262bC \\
        --tricrypto-v1-pool 0x960ea3e3C7FB317332d990873d354E18d7645590

Источники адресов: curvefi/twocrypto-ng 89c8dfb и curvefi/tricrypto-ng ecaa816 deployments.yaml (arbitrum:mainnet);
curvefi/curve-factory-crypto 7ed1011 deployment-logs/others.log; curvefi/curve-crypto-contract d7d04cd
«deployment-logs/2021-09-13. Arbitrum/log.txt». Сигнатуры вызовов — исходники пулов (src/venues/bsc_dex/pools/
abi_crypto.rs, abi_crypto_v1.rs).
"""

import argparse
import datetime
import hashlib
import json
import os
import sys

from curve_crypto_pools import LADDER_PPM, code, factory_pools, ng_rows, v1_row
from curve_crypto_rpc import Node, selector
from curve_sim import SENDER, Sim

def split(v):
    return [a for a in (v or "").split(",") if a]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--twocrypto-factory", required=True)
    ap.add_argument("--twocrypto-math", required=True, help="math v2.1.0 (deployments.yaml math)")
    ap.add_argument("--twocrypto-math-v200", required=True, help="math v2.0.0 (deployments.yaml math_v2_0_0)")
    ap.add_argument("--twocrypto-impls", default="", help="реализации пула через запятую (amm, amm_v2_0_0)")
    ap.add_argument("--tricrypto-factory", required=True)
    ap.add_argument("--tricrypto-math", required=True)
    ap.add_argument("--tricrypto-impls", default="", help="amm_native_transfers_enabled, …_disabled")
    ap.add_argument("--v1-factory", required=True, help="фабрика crypto v1 (others.log)")
    ap.add_argument("--v1-template", required=True, help="шаблон пула фабрики crypto v1 (others.log)")
    ap.add_argument("--tricrypto-v1-pool", required=True, help="tricrypto 2021 (log.txt)")
    ap.add_argument("--crypto2-v1-pool", default=None, help="T472-10: crypto-пул из 2 монет без фабрики (EURS/2CRV)")
    ap.add_argument("--code-pools", default="", help="T472-10: пулы без исходника — только eth_getCode (через запятую)")
    ap.add_argument("--max-pools", type=int, default=20, help="пулов на каждую математику / фабрику v1")
    ap.add_argument("--out", default=None, help="папка (по умолчанию tests/data/arbitrum_curve_crypto_<дата>)")
    a = ap.parse_args()

    node = Node(a.node)
    # Решение 472 (T472-02): отправитель симуляций exchange (curve_sim.py) должен быть пуст.
    if not Sim(a.node, "latest").empty():
        sys.exit("ОШИБКА: отправитель симуляции %s не пуст" % SENDER)
    chain = node.rpc("eth_chainId", [])["result"]
    head = int(node.rpc("eth_blockNumber", [])["result"], 16)
    tag = hex(head)
    blk = node.rpc("eth_getBlockByNumber", [tag, False])["result"]
    date = datetime.date.today().isoformat()
    out = a.out or os.path.join(os.path.dirname(__file__), "..", "..", "tests", "data", f"arbitrum_curve_crypto_{date}")
    os.makedirs(out, exist_ok=True)
    base = {"chain_id": chain, "block": head, "block_hash": blk["hash"], "timestamp": int(blk["timestamp"], 16)}

    files = {}
    specs = (
        ("twocrypto", a.twocrypto_factory, 2, [a.twocrypto_math, a.twocrypto_math_v200], split(a.twocrypto_impls)),
        ("tricrypto", a.tricrypto_factory, 3, [a.tricrypto_math], split(a.tricrypto_impls)),
    )
    for kind, factory, n, maths, impls in specs:
        count, rows = ng_rows(node, factory, n, maths, a.max_pools, tag)
        files[f"{kind}.json"] = dict(base, kind=kind, factory=factory, pool_count=count, maths=maths,
                                     code_of={x: code(node, x, tag) for x in maths + impls}, pools=rows)

    count, at = factory_pools(node, a.v1_factory, tag)
    v1 = [v1_row(node, p, 2, tag, a.v1_factory) for p in (at(k) for k in list(reversed(range(count)))[: a.max_pools]) if p]
    # Решение 462: pool_count = 0 и откат вызова раньше не различались (factory_pools: «or 0») — сырой ответ и код
    # фабрики пишутся рядом. Прогон 20.11: 0; Curve API getPools/arbitrum/factory-crypto — тоже 0 пулов.
    files["crypto_v1.json"] = dict(base, kind="crypto_v1", factory=a.v1_factory, pool_count=count,
                                   pool_count_raw=node.call(a.v1_factory, selector("pool_count()"), tag),
                                   templates=[a.v1_template],
                                   code_of={x: code(node, x, tag) for x in (a.v1_template, a.v1_factory)}, pools=v1)
    files["tricrypto_v1.json"] = dict(base, kind="tricrypto_v1", pools=[v1_row(node, a.tricrypto_v1_pool, 3, tag)])
    # Решение 472 (T472-10): пара без фабрики (вид curve_crypto2_v1) — те же вызовы v1 + is_killed(); код пулов без
    # исходника (сверка с кодом 2pool — для разбора, не для подключения).
    if a.crypto2_v1_pool:
        files["crypto2_v1.json"] = dict(base, kind="crypto2_v1",
                                        pools=[v1_row(node, a.crypto2_v1_pool, 2, tag, pair=True)])
    if a.code_pools:
        files["code_pools.json"] = dict(base, kind="code_pools", code_of={p: code(node, p, tag) for p in split(a.code_pools)})

    for name, doc in files.items():
        with open(os.path.join(out, name), "w") as f:
            json.dump(doc, f, indent=1)

    lines = [
        f"# Curve crypto-пулы Arbitrum One — запись своего узла, {date}",
        "",
        "Скрипт: tools/arbitrum_record/curve_crypto.py (только eth_call / eth_getCode / eth_getStorageAt / eth_getBalance / eth_getBlockByNumber, без ключей).",
        f"Узел: chainId {chain}; блок {head} ({blk['hash']}), время блока {base['timestamp']}.",
        "",
        "Что записано (на одном блоке): ответы на вызовы бота по calldata (pools/load_crypto.rs, load_crypto_v1.rs),",
        "version(), MATH(), get_coins (и get_decimals у v1) фабрики, код (eth_getCode) пулов, реализаций и математики,",
        "decimals() монет v1, balanceOf(pool) монет и eth_getBalance(pool) (holdings),",
        "у пулов фабрики crypto v1 — tweak_v1 (решение 472, T472-03): token(), factory(), price_oracle(), xcp_profit(),",
        "xcp_profit_a(), virtual_price(), allowed_extra_profit(), adjustment_step(), admin_fee(), totalSupply() LP,",
        "fee_receiver() фабрики, get_eth_index фабрики, слоты 28, 31-34 (33 — not_adjusted) и их сверка с геттерами,",
        f"лесенка get_dy(i, j, dx) в обе стороны (dx — доли баланса, ppm: {LADDER_PPM}).",
        f"Решение 472: exchange_sim — eth_call exchange на тех же суммах с подменой баланса и разрешения у {SENDER}.",
        "Пулы фабрик — с конца pool_list. Откат вызова — поле error.",
        "Читают тесты tests/arb_curve_crypto_live.rs (NG) и tests/arb_curve_crypto_v1_live.rs (v1) — снять #[ignore].",
        "",
        "## SHA-256 файлов",
    ]
    for name in sorted(files):
        with open(os.path.join(out, name), "rb") as f:
            lines.append(f"{hashlib.sha256(f.read()).hexdigest()}  {name}")
    with open(os.path.join(out, "README.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print(f"записано в {out}", file=sys.stderr)


if __name__ == "__main__":
    main()
