"""Check WhiteSwap's complete Ethereum factory, deployed proxy implementations and V5 payments at one block."""
import json
import sys
from pathlib import Path
import requests
from eth_utils import keccak

from record_balancer_markets import Capture
from record_ethereum_v2_factories import directory, digest, raw
from record_whiteswap_wallets import FIXTURE, SLOT, request

def capture():
    proof=json.loads(FIXTURE.read_bytes())
    tag=proof["selector"]
    expected=proof["code_hashes"]
    base="https://eth.blockscout.com/api/v2/smart-contracts/"
    factory=proof["factory"]
    def verified(address):
        r=requests.get(base+address,timeout=30);r.raise_for_status();d=r.json()
        if not d["is_verified"] or d["is_changed_bytecode"] or digest(raw(d["deployed_bytecode"]))!=expected[address]:
            raise ValueError("verified code differs from archival code: "+address)
        return d
    first=verified(proof["pool"])
    pair_impl=verified(proof["sources"]["pool"]["implementation"])
    factory_impl=verified(proof["sources"]["factory"]["implementation"])
    for phrase,src in [(b'type(WSProxyPair).creationCode',factory_impl),(b'_swap(',pair_impl)]:
        if phrase not in src["source_code"].encode():
            raise ValueError("unknown implementation source")
    init=raw(first["creation_bytecode"])
    fcode=raw(factory_impl["deployed_bytecode"])
    if init not in fcode:
        raise ValueError("pair CREATE2 initcode not present in verified factory implementation")
    origin=dict(url=base+proof["sources"]["factory"]["implementation"],runtime=expected[factory],
        creation_code_hash=digest(init),codecopy=[fcode.index(init),len(init)],sources=proof["sources"],
        code_hashes=expected)
    for url in ("https://eth.drpc.org","https://ethereum-rpc.publicnode.com","https://mainnet.gateway.tenderly.co"):
        try:
            head=request(url,"eth_getBlockByNumber",[hex(proof["block"]),False])
            if (request(url,"eth_chainId",[])!="0x1" or head["hash"]!=tag["blockHash"]
                    or head["stateRoot"]!=proof["state_root"]):raise ValueError("wrong archive")
            c=Capture(dict(endpoint=url,selector=tag))
            data=directory("whiteswap",c,config=(factory,10000,30,"WSwap: K"),proof=origin)
            def guard(proxy,role):
                ref=proof["sources"][role]
                word=raw(request(url,"eth_getStorageAt",[proxy,SLOT,tag]))
                impl=ref["implementation"]
                return dict(pool=proxy,implementation="0x"+word[-20:].hex(),
                    runtime=digest(raw(request(url,"eth_getCode",[proxy,tag]))),
                    implementation_hash=digest(raw(request(url,"eth_getCode",["0x"+word[-20:].hex(),tag]))),
                    slot_canonical=(len(word)==32 and word[:12]==bytes(12)))
            guards={role:guard(proof["sources"][role]["proxy"],role) for role in ("factory","router")}
            guards["pairs"]=[guard(row["pool"],"pool") for row in data["pools"]]
            for role,rows in [("factory",[guards["factory"]]),("router",[guards["router"]]),("pool",guards["pairs"])]:
                ref=proof["sources"][role]
                for row in rows:
                    row["matches_reference"]=(row["slot_canonical"] and row["implementation"]==ref["implementation"]
                        and row["runtime"]==expected[ref["proxy"]] and row["implementation_hash"]==expected[ref["implementation"]])
            return dict(data,selector=tag,height=proof["block"],state_root=head["stateRoot"],proxies=guards)
        except (RuntimeError,ValueError,KeyError,requests.RequestException) as e:
            print(f"{url}: {str(e)[:180]}",flush=True)
    raise ValueError("no archive returned the complete WhiteSwap registry")

if __name__=="__main__":
    Path(sys.argv[1]).write_text(json.dumps(capture(),indent=2)+"\n",encoding="utf8")
