"""Canonical Base RFQ registry and an actual, price-adjusted same-chain payment.
Settlement source: Dexalot/contracts@993df50c014f3c8a1a33e45b4364ab9e709662fb contracts/MainnetRFQ.sol.
Public API: Dexalot/dexalot-docs@a2a1ec62007cc2513a9099a8cec59a7da5487712 docs/en/apiv2/SimpleSwap.md.
Only read RPC/replay. No transaction or firm quote is submitted to the venue.
"""
import json
import sys
from pathlib import Path

from eth_abi import decode, encode
from eth_utils import keccak
import requests

from record_base_slipstream_executor import ROOT
from record_bitswap_ethereum import balance, token_flow

ROUTER="0xf00240e5256e72771b46d095666594e0f40d085c"
SLOT="0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
WITNESS="0x8cf90507a0f102969f7a3f5fcdddb82098987924ac4efe918b7971eb8ebc6b04"
ORDER="(uint256,uint128,address,address,address,address,uint256,uint256)"
EVENT="0x"+keccak(text="SwapExecuted(uint256,address,bytes32,uint32,address,bytes32,uint256,uint256)").hex()
USDC="0x833589fcd6edb6e08f4c7c32d4f71b54bda02913"
WETH="0x4200000000000000000000000000000000000006"


class Read:
    def __init__(self):
        self.session=requests.Session()

    def __call__(self,method,args):
        for url in ("https://mainnet.base.org","https://base-rpc.publicnode.com","https://base.gateway.tenderly.co"):
            try:
                r=self.session.post(url,json={"id":1,"jsonrpc":"2.0","method":method,"params":args},timeout=15)
                r.raise_for_status()
                obj=r.json()
                if obj.get("result") is not None:
                    return obj["result"]
                reason=str(obj.get("error"))[:140]
            except (ValueError,requests.RequestException) as err:
                reason=str(err)[:140]
        raise RuntimeError((method,reason))


def call(r,at,a,method,ret,types=(),args=()):
    data="0x"+(keccak(text=method)[:4]+encode(types,args)).hex()
    return decode([ret],bytes.fromhex(r("eth_call",[{"to":a,"data":data},at])[2:]))[0]


def role(r,a,at):
    def runtime(address):
        code=bytes.fromhex(r("eth_getCode",[address,at])[2:])
        assert code
        return dict(address=address,hash="0x"+keccak(code).hex(),bytes=len(code))
    d=runtime(a)
    slot=int(r("eth_getStorageAt",[a,SLOT,at]),16)
    assert slot<2**160
    if slot:
        d["implementation"]=runtime("0x"+slot.to_bytes(20,"big").hex())
    d["version"]=call(r,at,a,"VERSION()","bytes32").rstrip(bytes(1)).decode("ascii")
    return d


def acquire():
    cfg=json.loads((ROOT/"tests/data/base_ethereum_networks.json").read_bytes())["base"]
    at={"blockHash":cfg["hash"],"requireCanonical":True}
    rr=Read()
    assert int(rr("eth_chainId",[]),16)==cfg["chain"]
    header=rr("eth_getBlockByNumber",[hex(cfg["block"]),False])
    assert header["hash"]==cfg["hash"]
    members=call(rr,at,ROUTER,"getAllowedRFQs()","address[]")
    assert len(members)==len(set(members)) and members
    roles={a:role(rr,a,at) for a in (ROUTER,*members)}
    # Fixed 43,200 two-second block intervals before the snapshot, including both endpoints.
    first=cfg["block"]-43200
    start=rr("eth_getBlockByNumber",[hex(first),False])
    events=[]
    for lo in range(first,cfg["block"]+1,5000):
        logs=rr("eth_getLogs",[{"fromBlock":hex(lo),"toBlock":hex(min(lo+4999,cfg["block"])),"topics":[EVENT],"address":list(members)}])
        events.extend(logs)
    assert rr("eth_getBlockByNumber",[hex(cfg["block"]),False])["hash"]==at["blockHash"]
    start_ts,end_ts=map(lambda h:int(h["timestamp"],16),(start,header))
    return dict(head=at,height=cfg["block"],roles=roles,router=ROUTER,members=members,
                interval=dict(first=first,last=cfg["block"],start_time=start_ts,end_time=end_ts,swap_events=len(events)),
                witness=payment(rr))


def payment(r):
    tx=r("eth_getTransactionByHash",[WITNESS]); receipt=r("eth_getTransactionReceipt",[WITNESS])
    block=r("eth_getBlockByNumber",[receipt["blockNumber"],False])
    assert tx["hash"]==receipt["transactionHash"]==WITNESS and tx["blockHash"]==receipt["blockHash"]==block["hash"]
    assert block["transactions"][int(receipt["transactionIndex"],16)]==WITNESS and tx["to"]==ROUTER and int(receipt["status"],16)==1
    amount=bytes.fromhex(tx["input"][2:])
    assert amount[:4]==keccak(text="simpleSwap("+ORDER+",bytes)")[:4]
    order,sig=decode([ORDER,"bytes"],amount[4:])
    nonce,expiry,out,src,maker,taker,nominal,spent=order
    payee="0x"+(nonce>>96).to_bytes(20,"big").hex()
    assert len(sig)==65 and payee==taker==tx["from"] and out==WETH and src==USDC
    event,=[l for l in receipt["logs"] if l["address"]==maker and l["topics"][0]==EVENT]
    assert int(event["topics"][1],16)==nonce
    actor,dest,chain,a,b,debit,net=decode(["address","bytes32","uint32","address","bytes32","uint256","uint256"],bytes.fromhex(event["data"][2:]))
    assert actor==taker and int.from_bytes(dest,"big")==int(payee,16) and chain==8453 and a==src and int.from_bytes(b,"big")==int(out,16)
    assert (debit,net)==(-token_flow(receipt["logs"],src,taker),token_flow(receipt["logs"],out,payee))
    assert spent==debit and net<nominal
    at={"blockHash":block["parentHash"],"requireCanonical":True}
    hi=int(block["timestamp"],16)
    slip=(nonce>>88)&255
    assert 7<slip<128 and hi<=expiry
    key=((hi-(expiry-5*(slip>>3)))<<3) | (slip&7)
    factor=call(r,at,maker,"slippagePoints(uint256)","uint24",["uint256"],[key])
    assert factor>1 and nominal*(1_000_000-factor)//1_000_000==net
    frame=[balance(src,taker),balance(out,payee)]
    exec_call={"from":taker,"to":ROUTER,"data":tx["input"],"gas":tx["gas"],"value":tx["value"]}
    simulated=r("eth_simulateV1",[{"blockStateCalls":[{"blockOverrides":{"time":hex(hi)},"calls":frame+[exec_call,exec_call]+frame}]},at])
    assert len(simulated)==1 and simulated[0]["parentHash"]==at["blockHash"]
    rows=simulated[0]["calls"];assert [x["status"] for x in rows]==["0x1","0x1","0x1","0x0","0x1","0x1"]
    changes=[int(rows[i+4]["returnData"],16)-int(rows[i]["returnData"],16) for i in [0,1]]
    assert changes==[-spent,net]
    return dict(tx=WITNESS,block=int(block["number"],16),block_hash=block["hash"],parent_hash=block["parentHash"],
                timestamp=hi,expiry=expiry,router=role(r,ROUTER,at),maker=role(r,maker,at),taker=taker,input=src,output=out,
                nominal=str(nominal),spent=str(spent),cash=str(net),slip_key=key,slip_ppm=factor,nonce=str(nonce),
                actual_gas=int(receipt["gasUsed"],16),replay_gas=int(rows[2]["gasUsed"],16),replay_deltas=list(map(str,changes)),
                repeated=rows[3].get("error"),logs=receipt["logs"])


if __name__=="__main__":
    Path(sys.argv[1]).write_text(json.dumps(acquire(),separators=(",",":")),encoding="utf8")
