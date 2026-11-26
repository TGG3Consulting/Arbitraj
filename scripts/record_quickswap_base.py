"""Проверяет канонические размещения QuickSwap V4 Algebra и исполняет обе стороны по сохранённым пулам."""
import hashlib
import re
import sys
from pathlib import Path

import requests
from eth_utils import keccak

from record_hydrex_integral import (Capture, EXEC, FOLDER, QUOTE, RETURN, RUNTIME, SIZES, VIRTUAL, VIRTUAL_FILE,
                                    ZERO, code, map_ticks, read, rpc, simulate, verified, write)
from record_solidly_v3 import HOLDERS

DOC = "https://docs.quickswap.exchange/overview/contracts-and-addresses.md"
BEACON_SLOT = hex(int.from_bytes(keccak(text="eip1967.proxy.beacon"))-1)
STABLE = "0xfde4c96c8593536e31f229ea8f37b2ada2699bb2"


def main(folder):
    e = read(FOLDER/"base.json")
    c = Capture(e)
    text = requests.get(DOC, timeout=20)
    text.raise_for_status()
    section = text.text.split("BASE V4 Algebra",1)[1].split("</table>",1)[0]
    addr = {k:a.lower() for k,a in re.findall(r"<td>([^<]+)</td><td>(0x[0-9a-fA-F]{40})</td>",section)}
    factory,q = addr["AlgebraFactory"],addr["QuoterV2"]
    active, = c.one(factory,"defaultPluginFactory()",returns=["address"])
    if c.one(active,"algebraFactory()",returns=["address"])[0]!=factory:
        raise ValueError("текущая фабрика плагинов указывает на другой протокол")
    code(e,active)
    code(e,addr["PluginFactory"])
    if c.one(factory,"poolDeployer()",returns=["address"])[0]!=addr["AlgebraPoolDeployer"]:
        raise ValueError("другая фабрика или развёртыватель")
    samples = read(folder / "base-integration-2026-11-26/sources/sample-pools.json")["quickswap-v4-base"]["samples"]
    state = {EXEC:"0x"+RUNTIME.read_text().strip(),VIRTUAL:"0x"+VIRTUAL_FILE.read_text().strip()}
    header = rpc(e,"eth_getBlockByHash",[e["selector"]["blockHash"],False])
    deadline = int(header["timestamp"],16)+300
    sizes = {**SIZES,STABLE:[10**6,10**8]}
    sources = {q:verified(e,q,"QuoterV2"),factory:verified(e,factory,"AlgebraFactory")}
    markets = []
    proxy_template = None
    for row in samples:
        pool = row["address"].lower()
        tokens = [c.one(pool,kind+"()",returns=["address"])[0] for kind in ("token0","token1")]
        if (c.one(factory,"poolByPair(address,address)",["address","address"],tokens,["address"])[0]!=pool or
                c.one(pool,"factory()",returns=["address"])[0]!=factory):
            raise ValueError("пул отсутствует в реестре")
        sources[pool] = verified(e,pool,"AlgebraPool")
        gs = c.one(pool,"globalState()",returns=["uint160","int24","uint16","uint8","uint16","bool"])
        plugin, = c.one(pool,"plugin()",returns=["address"])
        if c.one(active,"pluginByPool(address)",["address"],[pool],["address"])[0]!=plugin:
            raise ValueError("текущий реестр не знает плагин")
        previous, = c.one(addr["PluginFactory"],"pluginByPool(address)",["address"],[pool],["address"])
        proxy_code = bytes.fromhex(rpc(e,"eth_getCode",[plugin,e["selector"]])[2:])
        if proxy_template is None:
            sources[plugin] = verified(e,plugin,"AlgebraPluginProxy")
            url = sources[plugin]["url"].split("?")[0]
            response = requests.get(url,params={"fields":"runtimeBytecode.transformations,runtimeBytecode.transformationValues"},timeout=15)
            response.raise_for_status()
            compiler = response.json()["runtimeBytecode"]
            immutable, = compiler["transformations"]
            value = compiler["transformationValues"]["immutables"][immutable["id"]]
            if immutable["reason"]!="immutable" or int(value,16)!=int(pool,16): raise ValueError("другое immutable")
            offset = immutable["offset"]
            proxy_template = proxy_code
            template_url = url
        if (proxy_code != proxy_template[:offset] + int(pool,16).to_bytes(32) + proxy_template[offset+32:]):
            raise ValueError("код прокси отличается вне неизменяемой привязки к пулу")
        if plugin not in sources:
            sources[plugin] = dict(template=template_url,immutable_offset=offset,pool=pool,match="immutable-only")
        code(e,plugin)
        if c.one(plugin,"pool()",returns=["address"])[0]!=pool:
            raise ValueError("прокси плагина привязан к другому пулу")
        beacon = "0x"+rpc(e,"eth_getStorageAt",[plugin,BEACON_SLOT,e["selector"]])[-40:]
        impl, = c.one(beacon,"implementation()",returns=["address"])
        if (c.one(active,"beacon()",returns=["address"])[0]!=beacon or
                c.one(active,"implementation()",returns=["address"])[0]!=impl):
            raise ValueError("реестр реализации не совпал с прокси")
        for address in [beacon,impl]: code(e,address)
        fee, = c.one(pool,"fee()",returns=["uint16"])
        liquidity, = c.one(pool,"liquidity()",returns=["uint128"])
        m = dict(pool=pool,tokens=tokens,price=str(gs[0]),tick=gs[1],last_fee=gs[2],config=gs[3],unlocked=gs[5],
                 current_fee=fee,plugin=plugin,previous_plugin=previous,beacon=beacon,implementation=impl,liquidity=str(liquidity),cases=[])
        for i,t in enumerate(tokens):
            sender = HOLDERS["base"][t]
            if rpc(e,"eth_getCode",[sender,e["selector"]])!="0x": raise ValueError("отправитель не EOA")
            for dx in sizes[t]:
                native = c.one(q,"quoteExactInputSingle("+QUOTE+")",[QUOTE],[(t,tokens[1-i],ZERO,dx,0)],RETURN)
                case = simulate(e,pool,q,tokens[0],t,tokens[1-i],dx,state,deadline,sender)
                case["at_block"] = [str(x) for x in native]
                m["cases"].append(case)
        m.update(map_ticks(c,pool,[m["tick"],*(case["tick_after"] for case in m["cases"])]))
        markets.append(m)
    selected = {a for m in markets for a in (m["pool"],m["plugin"],m["beacon"],m["implementation"])} | {q,factory,active,addr["PluginFactory"]}
    model = bytearray(proxy_template)
    model[offset:offset+32] = bytes(32)
    write(FOLDER/"quickswap_base.json",dict(chain_id=e["chain_id"],selector=e["selector"],endpoint=e["endpoint"],
        documentation=DOC,addresses=addr,active_plugin_factory=active,proxy_template_hash="0x"+keccak(model).hex(),
        sources=sources,code={a:e["contracts"][a] for a in sorted(selected)},
        runtime_sha256={a:hashlib.sha256(bytes.fromhex(s[2:])).hexdigest() for a,s in state.items()},
        virtual=VIRTUAL,executor=EXEC,markets=markets,calls=c.calls))
    print([(m["pool"],m["config"],m["current_fee"],len(m["cases"])) for m in markets])

if __name__=="__main__":
    main(Path(sys.argv[1]))
