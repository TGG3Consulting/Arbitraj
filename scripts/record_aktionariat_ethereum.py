"""Bind Aktionariat registrations and original shareholder settlements to a canonical Ethereum state."""
import argparse
import json
from pathlib import Path

import requests
import rlp
from eth_abi import decode, encode
from eth_utils import keccak

from fork_fee_rpc import Reader
from record_base_slipstream_executor import ROOT
from record_llama_base_cl import CanonicalCalls
from v3_factory_storage import map_slot, nibbles, prove_account, trie_node, word

SOURCES = ("https://github.com/aktionariat/contracts/tree/c2d3dd438aff52185ea7c1ec6ae821061075a41b/contracts/brokerbot",
           "https://github.com/aktionariat/contracts/tree/fe169a933fc151a4c4769c6387ea5e6f97f02447/contracts")
REGISTRIES = ("0xcb3e482df38d62e73a7ae0e15a2605cadcc5ae98", "0xc40ada4340767e5a08138f0095f18a1c4e805a70")
REGISTER = "0x"+keccak(text="RegisterBrokerbot(address,address,address)").hex()
TRADE = "0x"+keccak(text="Trade(address,address,bytes,int256,address,uint256,uint256,uint256)").hex()
AGG = "0xca11bde05977b3631167028862be2a173976ca11"
ZERO = "0x"+"00"*20


def original_logs(reader, height, topic, start=0, address=None):
    query = dict(module="logs",action="getLogs",fromBlock=start,toBlock=height,topic0=topic,page=1,offset=1000)
    if address:
        query["address"] = address
    matches = {}
    while True:
        response=requests.get("https://api.routescan.io/v2/network/mainnet/evm/1/etherscan/api",params=query,timeout=30)
        response.raise_for_status()
        entries=response.json()["result"]
        if not isinstance(entries,list):
            raise ValueError(("unexpected event response",str(entries)[:100]))
        for entry in entries:
            key=(entry["blockHash"],int(entry["logIndex"],16))
            assert key not in matches and entry["topics"][0]==topic
            matches[key] = entry
        if len(entries)<query["offset"]:
            break
        query["page"]+=1
    # Verify the original event data, block number and transaction against the chain.
    blocks=sorted({int(l["blockNumber"],16) for l in matches.values()})
    headers=reader.many(("eth_getBlockByNumber",[hex(h),False]) for h in blocks)
    queries=[dict(blockHash=h["hash"],topics=[topic],**({"address":address} if address else {})) for h in headers]
    originals=reader.many(("eth_getLogs",[q]) for q in queries)
    checked=[]
    for header,rows in zip(headers,originals,strict=True):
        for row in rows:
            assert row["blockHash"]==header["hash"] and not row.get("removed")
            candidate=matches.pop((row["blockHash"],int(row["logIndex"],16)))
            assert all(candidate[k].lower()==row[k].lower() for k in ("data","address","transactionHash"))
            assert row["topics"]==candidate["topics"]
            checked.append(row)
    assert not matches
    return checked


def market_views(cap,queries):
    values=[]
    for i in range(0,len(queries),48):
        batch=queries[i:i+48]
        packed=[(address,True,keccak(text=sig)[:4]+encode(types,args)) for address,sig,types,args,typ in batch]
        returned,=cap.one(AGG,"aggregate3((address,bool,bytes)[])",["(address,bool,bytes)[]"],[packed],["(bool,bytes)[]"])
        for (success,data),query in zip(returned,batch,strict=True):
            values.append(decode([query[4]],data)[0] if success and len(data)==32 else None)
    return values


def record():
    head=json.loads((ROOT/"tests/data/first_run_dex_2026-11-26/ethereum.json").read_bytes())
    reader=Reader("ethereum",head);cap=CanonicalCalls(reader)
    directories=[];pairs={}
    for registry in REGISTRIES:
        rows=original_logs(reader,head["block"],REGISTER,address=registry)
        latest={}
        for row in sorted(rows,key=lambda v:(int(v["blockNumber"],16),int(v["logIndex"],16))):
            base,share=[decode(["address"],bytes.fromhex(t[2:]))[0] for t in row["topics"][1:]]
            latest[base,share]=decode(["address"],bytes.fromhex(row["data"][2:]))[0]
        proof=reader.one("eth_getProof",[registry,[],hex(head["block"])])
        verified=prove_account(reader.header,proof,registry)
        metadata=requests.get(f"https://sourcify.dev/server/v2/contract/1/{registry}",
                              params={"fields":"storageLayout,runtimeBytecode.onchainBytecode"},timeout=20)
        metadata.raise_for_status(); compiled=metadata.json()
        assert {r["label"]:r["slot"] for r in compiled["storageLayout"]["storage"]}=={"owner":"0","getBrokerbot":"1"}
        assert "0x"+keccak(hexstr=compiled["runtimeBytecode"]["onchainBytecode"]).hex()==proof["codeHash"]
        owner,=cap.one(registry,"owner()",returns=["address"])
        slots={0:int(owner,16)} if int(owner,16) else {}
        calls=[(registry,"getBrokerbot(address,address)",["address","address"],list(pair),"address") for pair in latest]
        for (cash,share),live in zip(latest,market_views(cap,calls),strict=True):
            assert live==latest[cash,share]
            if live!=ZERO:
                slots[map_slot(int(share,16),map_slot(int(cash,16),1))]=int(live,16)
                pairs[live]=(cash,share)
        leaves=[(nibbles(keccak(word(k))),rlp.encode(v)) for k,v in slots.items()]
        assert keccak(rlp.encode(trie_node(leaves)))==verified if leaves else keccak(rlp.encode(b""))==verified
        directories.append(dict(registry=registry,registry_runtime=proof["codeHash"],owner=owner,events=rows,
                                storage_root=proof["storageHash"],nonce=proof["nonce"],proof=proof["accountProof"],slots=len(slots)))
    start=head["block"]-7200
    trades=original_logs(reader,head["block"],TRADE,start)
    active=sorted({e["address"].lower() for e in trades})
    candidates=sorted(set(pairs)|set(active))
    metadata=market_views(cap,[(a,s,[],[],typ) for a in candidates for s,typ in
                              [("base()","address"),("token()","address"),("paymenthub()","address"),("settings()","uint256")]])
    markets={}; queries=[]
    for i,a in enumerate(candidates):
        cash,share,hub,flags=metadata[i*4:i*4+4]
        market=dict(address=a,cash=cash,share=share,hub=hub,settings=flags,registered_pair=pairs.get(a))
        markets[a]=market
        if cash and share and hub and flags is not None:
            queries.extend([(a,market,field,q) for field,q in [
                ("cash_balance",(cash,"balanceOf(address)",["address"],[a],"uint256")),
                ("shares",(share,"balanceOf(address)",["address"],[a],"uint256")),
                ("buy_one",(a,"getBuyPrice(uint256)",["uint256"],[1],"uint256")),
                ("sell_one",(a,"getSellPrice(uint256)",["uint256"],[1],"uint256")),
                ("share_decimals",(share,"decimals()",[],[],"uint8"))]])
    values=market_views(cap,[q[3] for q in queries])
    for (_,market,field,_),v in zip(queries,values,strict=True):
        market[field]=None if v is None else str(v)
    roles=sorted(set(candidates)|{m["hub"] for m in markets.values() if m["hub"]})
    code=reader.many(("eth_getCode",[a,reader.tag]) for a in roles)
    hashes={a:{"bytes":len(c[2:])//2,"runtime":"0x"+keccak(hexstr=c).hex()} for a,c in zip(roles,code,strict=True)}
    txs=sorted({l["transactionHash"] for l in trades})
    transactions=reader.many(("eth_getTransactionByHash",[h]) for h in txs)
    transfers=[]
    txmap={h:t for h,t in zip(txs,transactions,strict=True)}
    for log in trades:
        tx=txmap[log["transactionHash"]]
        assert tx["blockHash"]==log["blockHash"]
        payer,reference,units,cash,gross,fee,price=decode(["address","bytes","int256","address","uint256","uint256","uint256"],bytes.fromhex(log["data"][2:]))
        transfers.append(dict(market=log["address"].lower(),tx=log["transactionHash"],block=log["blockHash"],
          number=int(log["blockNumber"],16),log_index=int(log["logIndex"],16),target=tx["to"],method=tx["input"][:10],
          cash=cash,share="0x"+log["topics"][1][-40:],amount=str(units),price_after=str(price),total=str(gross),fee=str(fee),
          reference_code=reference[:1].hex(),payer=payer))
    return dict(height=head["block"],selector=reader.tag,state_root=reader.header["stateRoot"],sources=SOURCES,
                directories=directories,markets=markets,roles=hashes,trades=transfers,trade_from=start)


if __name__=="__main__":
    p=argparse.ArgumentParser(description=__doc__);p.add_argument("output",type=Path)
    p.parse_args().output.write_bytes(json.dumps(record(),separators=(",",":")).encode())
