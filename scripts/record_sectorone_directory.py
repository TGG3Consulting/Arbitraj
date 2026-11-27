"""Канонический реестр двух поколений LB SectorOne и их официальные роли."""
import argparse
import json
from pathlib import Path

import requests
from bs4 import BeautifulSoup
from eth_abi import decode
from eth_utils import keccak
from record_balancer_markets import Capture, posts
from record_base_slipstream_executor import ROOT, rpc
from record_ethereum_x7 import ENDPOINTS as ETH_ENDPOINTS
from record_omni_v2_base import ENDPOINTS as BASE_ENDPOINTS

DOC = "https://docs.sectorone.xyz/info/smart-contracts.md"


def roles(chain, version):
    page = requests.get(DOC, timeout=20)
    page.raise_for_status()
    name = "Base" if chain == "base" else "Ethereum Mainnet"
    section = page.text.split("## Smart Contracts on " + name + " <", 1)[1].split("##", 1)[0]
    rows = {cells[0].get_text(): cells[1].get_text().lower()
            for tr in BeautifulSoup(section, "html.parser").select("tr") if len(cells := tr.select("td")) == 2}
    return [rows[k + " (LB v" + version + ")"] for k in ["LBFactory", "LBPair Implementation", "LBRouter"]]


def capture(chain, version, endpoint):
    snap = json.loads((ROOT / "tests/data/first_run_dex_2026-11-26" / (chain + ".json")).read_bytes())
    c = Capture(dict(snap, endpoint=endpoint))
    legacy = version == "2.0"
    cid = snap["chain_id"]
    factory, impl, router = roles(chain, version)
    verified = {}
    for address in [factory, impl, router]:
        url = f"https://sourcify.dev/server/v2/contract/{cid}/{address}?fields=compilation,runtimeBytecode.onchainBytecode"
        reply = requests.get(url,timeout=20)
        if reply.status_code == 404 and chain == "ethereum":
            url = "https://eth.blockscout.com/api/v2/smart-contracts/" + address
            reply = requests.get(url, timeout=20)
            reply.raise_for_status()
            source = reply.json()
            deployed, name = source["deployed_bytecode"], source["name"]
            matched = source.get("is_verified") and not source.get("is_changed_bytecode")
        else:
            reply.raise_for_status()
            source = reply.json()
            deployed, name = source["runtimeBytecode"]["onchainBytecode"], source["compilation"]["name"]
            matched = bool(source.get("runtimeMatch"))
        code = rpc(endpoint,"eth_getCode",[address,c.selector])
        if code == "0x" or not matched or code.lower() != deployed.lower():
            raise ValueError(f"код размещения {address} расходится с исходником")
        verified[address] = dict(source=url, name=name, runtime="0x"+keccak(bytes.fromhex(code[2:])).hex())
    fac = "factory()" if legacy else "getFactory()"
    impl_name = "LBPairImplementation()" if legacy else "getLBPairImplementation()"
    if (c.one(router,fac,returns=["address"])[0],c.one(impl,fac,returns=["address"])[0],
            c.one(factory,impl_name,returns=["address"])[0]) != (factory,factory,impl):
        raise ValueError("нарушена привязка реализация/роутер/фабрика")
    count, = c.one(factory,"getNumberOfLBPairs()",returns=["uint256"])
    index = "allLBPairs(uint256)" if legacy else "getLBPairAtIndex(uint256)"
    pairs = [decode(["address"],b)[0] for b in posts(c,[(factory,index,["uint256"],[i]) for i in range(count)])]
    if len(set(pairs)) != count: raise ValueError("повтор пула в реестре")
    getters = ([fac,"tokenX()","tokenY()","feeParameters()","getReservesAndId()"] if legacy else
               [fac,"getTokenX()","getTokenY()","getBinStep()","getReserves()","implementation()","getLBHooksParameters()"])
    fields = posts(c,[(addr,sig,[],[]) for addr in pairs for sig in getters])
    rows = []
    for i,pool in enumerate(pairs):
        f,x,y,fee,reserves,*extra = fields[i*len(getters):(i+1)*len(getters)]
        if decode(["address"],f)[0] != factory: raise ValueError("чужая фабрика")
        x,y = (decode(["address"],v)[0] for v in (x,y))
        step = int.from_bytes(fee[:32],"big")
        rx,ry=decode(["uint256","uint256"],reserves[:64])
        row = dict(pool=pool,tokens=[x,y],step=step,reserves=[str(rx),str(ry)])
        if legacy:
            row["fee_parameters"] = [str(int.from_bytes(fee[i:i+32],"big")) for i in range(0,len(fee),32)]
        else:
            row["implementation"] = decode(["address"],extra[0])[0]
            row["hooks"] = "0x"+extra[1].hex()
        rows.append(row)
    confirmed=posts(c,[(factory,"getLBPairInformation(address,address,uint256)",["address","address","uint256"],[*r["tokens"],r["step"]]) for r in rows])
    for r,raw in zip(rows,confirmed,strict=True):
        bin_step,pair,owner,ignored=decode(["uint16","address","bool","bool"],raw)
        if (bin_step,pair)!=(r["step"],r["pool"]): raise ValueError("реестр не подтверждает токены/шаг")
        r.update(created_by_owner=owner,ignored_for_routing=ignored)
    return dict(chain_id=cid,chain=chain,version=version,documentation=DOC,selector=c.selector,
                factory=factory,implementation=impl,router=router,verified=verified,markets=rows,calls=c.calls)


if __name__=="__main__":
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("network",choices=["base","ethereum"])
    ap.add_argument("version",choices=["2.0","2.2"])
    ap.add_argument("output",type=Path)
    args=ap.parse_args()
    for endpoint in (BASE_ENDPOINTS if args.network=="base" else ETH_ENDPOINTS):
        try:
            directory=capture(args.network,args.version,endpoint)
            args.output.write_text(json.dumps(directory,indent=2)+"\n",encoding="utf8")
            break
        except (OSError,KeyError,ValueError) as exc:
            print(endpoint,str(exc)[:300],flush=True)
    else: raise ValueError("нет подтверждённого исходного реестра SectorOne")
