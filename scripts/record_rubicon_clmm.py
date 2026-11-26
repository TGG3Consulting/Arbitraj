"""Сверяет реестр и пулы Rubicon CLMM по каноническому блоку и байткоду создания пары."""
import argparse
import json
from pathlib import Path

import requests
from bs4 import BeautifulSoup
from eth_abi import encode
from eth_utils import keccak

from record_balancer_markets import Capture
from record_base_slipstream_executor import ROOT
from record_woo_base import rpc

DOC = "https://docs.rubicon.finance/developers/deployments"
INDEX = "https://api.routescan.io/v2/network/mainnet/evm/1/etherscan/api"
FOLDER = ROOT / "tests/data/first_run_dex_2026-11-26"
FEE = "0x" + keccak(text="FeeAmountEnabled(uint24,int24)").hex()
CREATED = "0x" + keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
ZERO = "0x" + "00" * 20


def fetch(url, **params):
    response = requests.get(url, params=params, timeout=25)
    response.raise_for_status()
    return response


def deployments(network):
    soup = BeautifulSoup(fetch(DOC).text, "html.parser")
    for heading in soup.select("h3,h4"):
        section = heading.find_previous("h2")
        if not section or section.get_text(strip=True) != "Rubicon CLMM Deployments":
            continue
        if heading.get_text(strip=True).lower() != network:
            continue
        return {row.select_one("td").get_text(strip=True):
                row.select("td a")[0]["href"].split("/")[-1].lower()
                for row in heading.find_next("table").select("tr") if row.select("td a")}
    raise ValueError(f"Официальная таблица Rubicon не содержит {network}")


def events(snapshot, factory):
    """Индекс служит для нахождения высот. Содержимое событий сверяется с RPC по хешу блока."""
    found = {}
    for topic in (FEE, CREATED):
        page = 1
        while True:
            data = fetch(INDEX, module="logs", action="getLogs", address=factory, topic0=topic,
                         fromBlock=0, toBlock=snapshot["block"], page=page, offset=100).json()
            if data.get("status") != "1" or not isinstance(data["result"], list):
                raise ValueError(f"Не получен индекс событий {topic}")
            for event in data["result"]:
                found[(int(event["blockNumber"], 16), int(event["logIndex"], 16))] = event
            if len(data["result"]) < 100:
                break
            page += 1
    result = []
    for height in sorted({h for h, _ in found}):
        block = rpc(snapshot, "eth_getBlockByNumber", [hex(height), False])
        logs = rpc(snapshot, "eth_getLogs", [{"address": factory, "blockHash": block["hash"],
                                             "topics": [[FEE, CREATED]]}])
        for log in logs:
            key = (height, int(log["logIndex"], 16))
            label = found.pop(key)
            if any(label[k].lower() != log[k].lower() for k in ("transactionHash", "data")) or label["topics"] != log["topics"]:
                raise ValueError("Содержание события не совпало с каноническим блоком")
            result.append(log)
    if found:
        raise ValueError("В канонических блоках отсутствуют события из индекса")
    return result


def pool_address(factory, tokens, fee, init_hash):
    salt = keccak(encode(["address", "address", "uint24"], [*tokens, fee]))
    return "0x" + keccak(b"\xff" + bytes.fromhex(factory[2:]) + salt + init_hash)[-20:].hex()


def record():
    capture = json.loads((FOLDER / "ethereum.json").read_bytes())
    capture["endpoint"] = "https://eth.drpc.org"
    c = Capture(capture)
    dep = deployments("ethereum")
    factory, lens = dep["RubiconV3Factory"], dep["TickLens"]
    ref = json.loads((FOLDER / "verified_v3_ethereum.json").read_bytes())["rubicon-clmm-ethereum"]["pool"]
    source = fetch(ref["source"], fields="creationBytecode.onchainBytecode").json()
    if not source.get("creationMatch"):
        raise ValueError("Нет проверенного байткода создания пары")
    init = keccak(bytes.fromhex(source["creationBytecode"]["onchainBytecode"][2:]))
    logs = events(capture, factory)
    tiers, pools, hashes = {}, [], {}
    for address in (factory, lens, dep["QuoterV2"]):
        runtime = bytes.fromhex(rpc(capture, "eth_getCode", [address, capture["selector"]])[2:])
        if not runtime:
            raise ValueError("Нет контракта из официального реестра")
        hashes[address] = {"bytes": len(runtime), "keccak256": "0x" + keccak(runtime).hex()}
    for event in logs:
        topics = event["topics"]
        if topics[0] == FEE:
            fee, spacing = (int(x, 16) for x in topics[1:])
            if c.one(factory, "feeAmountTickSpacing(uint24)", ["uint24"], [fee], ["int24"])[0] != spacing:
                raise ValueError("Уровень комиссии изменился")
            tiers[str(fee)] = spacing
            continue
        tokens = ["0x" + t[-40:] for t in topics[1:3]]
        fee = int(topics[3], 16)
        pool = "0x" + event["data"][-40:]
        spacing = int(event["data"][2:66], 16)
        for pair in (tokens, tokens[::-1]):
            deployed, = c.one(factory, "getPool(address,address,uint24)",
                              ["address","address","uint24"], [*pair,fee], ["address"])
            if deployed != pool:
                raise ValueError("Фабрика вернула другой контракт")
        if pool != pool_address(factory, tokens, fee, init) or tiers[str(fee)] != spacing:
            raise ValueError("Пул не соответствует CREATE2 проверенного байткода")
        if c.one(pool, "factory()", returns=["address"])[0] != factory:
            raise ValueError("Пул вернул другую фабрику")
        liquidity, = c.one(pool, "liquidity()", returns=["uint128"])
        pools.append(dict(address=pool,tokens=tokens,fee=fee,spacing=spacing,
                          liquidity=str(liquidity),height=int(event["blockNumber"],16),
                          block_hash=event["blockHash"],tx=event["transactionHash"]))
    slot = c.one(ref["pool"], "slot0()", returns=["uint160","int24","uint16","uint16","uint16","uint8","bool"])
    center = slot[1] // tiers[str(ref["fee"])] // 256
    ticks, = c.one(lens, "getPopulatedTicksInWord(address,int16)", ["address","int16"],
                   [ref["pool"],center], ["(int24,int128,uint128)[]"])
    if not any(p["address"] == ref["pool"] for p in pools):
        raise ValueError("Реестр не содержит рынок архивного исполнения")
    return dict(chain_id=1, selector=capture["selector"], deployment_source=DOC, event_index=INDEX,
                factory=factory, tick_lens=lens, quoter=dep["QuoterV2"], contracts=hashes,
                source=ref["source"], creation_hash="0x"+init.hex(), tiers=tiers, pools=pools,
                lens_sample={"pool":ref["pool"],"word":center,"ticks":[[t,str(n),str(g)] for t,n,g in ticks]})


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    path = parser.parse_args().output
    path.write_bytes((json.dumps(record(), ensure_ascii=False, indent=2)+"\n").encode("utf-8"))
