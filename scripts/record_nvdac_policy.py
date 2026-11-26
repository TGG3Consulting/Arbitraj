"""Привязывает реестр эмитента, код Base до контрольного блока и состояния B20 NVDAc к реальным участникам архива."""
import hashlib

import requests
from bs4 import BeautifulSoup
from eth_utils import keccak

from record_baibai import BOOK, ENTRY, FOLDER, USDC, VAULT, Capture, abi, available, read, write
from record_baibai_nvdac import COIN, HOLDER, SMALL_HOLDER
from record_base_slipstream_executor import EXEC
from record_lunarbase import SLOT

BASE = "a9f6cc9d1c9aaec5079a6451d06f67e9c0590b15"
STD = "b97145b22f7152505c2bdc448fff69d710528d9a"
FACTORY = "0xb20f000000000000000000000000000000000000"
POLICY = "0x8453000000000000000000000000000000000002"
ACTIVATION = "0x8453000000000000000000000000000000000001"


def capture():
    c = Capture(available(read(FOLDER/"base.json")))
    head=c.one("eth_getBlockByHash",[c.tag["blockHash"],False])
    assets="https://www.base.org/assets"
    prospectus="https://api.coinbase.com/v1/tokenized-stocks/prospectus/NVDAc.pdf"
    base=f"https://raw.githubusercontent.com/base/base/{BASE}/crates/common/precompiles/src/"
    std=f"https://raw.githubusercontent.com/base/base-std/{STD}/src/"
    urls=[assets,prospectus,"https://docs.base.org/upgrades/denim/overview.md",
          "https://docs.base.org/build-on-base/integrate-defi/list-tokenized-stocks.md",
          *[base+p for p in ("provider.rs","lookup.rs","b20_factory/variant.rs","b20_asset/logic/v3.rs","b20_asset/dispatch.rs")],
          std+"interfaces/IB20.sol",std+"StdPrecompiles.sol"]
    evidence={}
    for url in urls:
        r=requests.get(url,timeout=30);r.raise_for_status()
        evidence[url]=dict(sha256=hashlib.sha256(r.content).hexdigest(),bytes=len(r.content))
        if url==assets:
            root=BeautifulSoup(r.content,"html.parser")
            link=next(a for a in root.select("a[href]") if a["href"].lower().endswith(COIN))
            if prospectus not in [a["href"] for a in link.parent.parent.select("a[href]")]:
                raise ValueError("Адрес и проспект не в одной строке реестра эмитента")
    results={}
    def call(key,target,method,types=(),args=()):
        data=abi(method,types,args)
        response=c.one("eth_call",[{"to":target,"data":data},c.tag])
        results[key]=dict(to=target,data=data,result=response)
        return response
    for name in ("isB20","isB20Initialized"):
        if int(call(name,FACTORY,name,["address"],[COIN]),16)!=1:
            raise ValueError("Фабрика B20 не подтверждает токен")
    active=keccak(text="base.b20_asset")
    if int(call("active",ACTIVATION,"isActivated",["bytes32"],[active]),16)!=1:
        raise ValueError("Функция asset на сети выключена")
    call("transfer_paused",COIN,"isPaused",["uint8"],[0])
    call("ui_multiplier",COIN,"uiMultiplier")
    call("decimals",COIN,"decimals")
    cases=read(FOLDER/"baibai_nvdac_base.json")["cases"]
    people={r["owner"] for r in cases}|{HOLDER,SMALL_HOLDER,EXEC,ENTRY,VAULT,BOOK}
    scopes={}
    for name in ("TRANSFER_SENDER_POLICY","TRANSFER_RECEIVER_POLICY","TRANSFER_EXECUTOR_POLICY","SEIZE_EXEMPT_POLICY","SEIZE_RECEIVER_POLICY"):
        key=bytes.fromhex(call(name,COIN,name)[2:])
        policy=int(call(name+"_id",COIN,"policyId",["bytes32"],[key]),16)
        scopes[name]=policy
        for addr in sorted(people):
            call(name+":"+addr,POLICY,"isAuthorized",["uint64","address"],[policy,addr])
    code=c.one("eth_getCode",[COIN,c.tag])
    return dict(chain_id=8453,selector=c.tag,block=int(head["number"],16),timestamp=int(head["timestamp"],16),
                endpoint=c.url,token=COIN,variant="Asset",code=code,implementation_word=c.one("eth_getStorageAt",[COIN,SLOT,c.tag]),
                issuer="COINBASE ONCHAIN SPV LTD",prospectus_date="2026-08-04",sources=evidence,
                observed_addresses=sorted(people),scope_ids=scopes,reads=results)


if __name__=="__main__":
    write(FOLDER/"nvdac_policy_base.json",capture())
