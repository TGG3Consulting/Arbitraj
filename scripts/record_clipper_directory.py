"""Bind both Sushi Blade registries and old Clipper pools to the same canonical blocks."""
import argparse
import json
from pathlib import Path

import rlp
from eth_utils import keccak

from fork_fee_rpc import Reader
from record_aktionariat_ethereum import market_views, original_logs
from record_base_slipstream_executor import OWNER, ROOT, call, calldata, simulate_block
from record_llama_base_cl import CanonicalCalls
from v3_factory_storage import map_slot, nibbles, prove_account, trie_node, word

REVISION = "453c83478f2553270f4fcf52797695fc52553240"
DIRECT = {
    "ethereum": ["0x655edce464cc797526600a462a8154650eee4b77", "0xe7b0ce0526fbe3969035a145c9e9691d4d9d216c",
                 "0xcc12532e95c2a6a4c53af153b9b739a3cc9218a7"],
    "base": ["0xb32d856cad3d2ef07c94867a800035e37241247c","0x663fa8731045765067e2b5b6acb19be2ce974c7f"],
}
REGISTRY = {"ethereum": "0x2e32c76b4f50698f96fdd8ff4af0bd5d45f9399d", "base":"0x80a06bbab131eeeb78cac159f9ddecf8e9a6abf6"}
CREATED = ["0x"+keccak(text=f"Blade{kind}ExchangeCreated(address,address[],address[])").hex() for kind in ("Verified","Approximate")]
ZERO = "0x"+"00"*20
NATIVE = "0x"+"ee"*20
LEGACY = "0xf0f455e8b8f4f96ae5109493c5d3ea5e2c09de47"
COVE = "0x44d097113dbead613fde74b387081fb3b547c54f"
USDC = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48"
USDT = "0xdac17f958d2ee523a2206206994597c13d831ec7"
WETH = "0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2"

def root_check(reader, cap, registry, candidates):
    proof = reader.one("eth_getProof",[registry,[],hex(int(reader.header["number"],16))]); root = prove_account(reader.header,proof,registry)
    fields = ["owner", "bladeVerifiedImplementation", "permitRouterImplementation", "lpTransferImplementation"]
    values = [cap.one(registry, key+"()", returns=["address"])[0] for key in fields]
    slots = {i:int(v,16) for i,v in enumerate(values) if int(v,16)}
    registered = []
    for address, present in zip(candidates, market_views(cap, [(registry,"isRegistered(address)",["address"],[a],"bool") for a in candidates]),strict=True):
        if present is None: raise ValueError("registry read failed")
        if present:
            registered.append(address)
            slots[map_slot(int(address,16),4)]=1
    leaves = [(nibbles(keccak(word(k))),rlp.encode(v)) for k,v in slots.items()]
    reconstructed = keccak(rlp.encode(trie_node(leaves))) if leaves else keccak(rlp.encode(b""))
    return dict(address=registry, roles=dict(zip(fields,values)), registered=registered, complete=reconstructed==root,
        storage_root="0x"+root.hex(), reconstructed="0x"+reconstructed.hex(), account_proof=proof["accountProof"], runtime=proof["codeHash"])


def describe(reader,cap,pool):
    count, = cap.one(pool,"nTokens()", returns=["uint256"])
    assert 0<count<1024
    tokens = market_views(cap,[(pool,"tokenAt(uint256)",["uint256"],[i],"address") for i in range(count)])
    assert None not in tokens and len(set(tokens))==count
    erc20 = [t for t in tokens if t not in (ZERO,NATIVE)]
    cash = market_views(cap,[(t,"balanceOf(address)",["address"],[pool],"uint256") for t in erc20])
    if None in cash:raise ValueError("unreadable underlying balance")
    signer, halted = market_views(cap,[(pool,"DESIGNATED_SIGNER()",[],[],"address"),(pool,"isTradeHalted()",[],[],"bool")])
    code = reader.one("eth_getCode",[pool,reader.tag]); assert len(code)>2
    return dict(pool=pool,tokens=tokens,cash={t:str(v) for t,v in zip(erc20,cash)},
                native=str(int(reader.one("eth_getBalance",[pool,reader.tag]),16)),signer=signer,trade_halted=halted,runtime="0x"+keccak(hexstr=code).hex())


def unsigned(reader,cap):
    pool,exchange = [cap.one(LEGACY,k+"()",returns=["address"])[0] for k in ("clipperPool","clipperExchange")]
    assert cap.one(exchange,"theExchange()",returns=["address"])[0]==pool
    assert cap.one(pool,"exchangeInterfaceContract()",returns=["address"])[0]==exchange
    approval,=cap.one(exchange,"approvalContract()",returns=["address"])
    enabled,blocked,allowed=market_views(cap,[(approval,"swapsAllowed()",[],[],"bool"),(approval,"blocked(address)",["address"],[OWNER],"bool"),(approval,"approveSwap(address)",["address"],[OWNER],"bool")])
    assert isinstance(enabled,bool) and isinstance(blocked,bool) and allowed==(enabled and not blocked)
    data=describe(reader,cap,pool)
    quotes=[]
    # Genuine USDC acquired on a separate market. No ERC20 balance or allowance is overridden.
    uni="0x7a250d5630b4cf539739df2c5dacb4c659f2488d"
    deadline=int(reader.header["timestamp"],16)+120
    for size in (10**6,10**7):
        q,=market_views(cap,[(pool,"getSellQuote(address,address,uint256)",["address","address","uint256"],[USDC,USDT,size],"uint256")])
        fund=call(uni,calldata("swapETHForExactTokens(uint256,address[],address,uint256)",["uint256","address[]","address","uint256"],[size,[WETH,USDC],OWNER,deadline]),10**18)
        approve=call(USDC,calldata("approve(address,uint256)",["address","uint256"],[LEGACY,size]))
        swap=call(LEGACY,calldata("clipperSwap(address,uint256,address,address,uint256)",["address","uint256","address","address","uint256"],[USDC,size,USDT,OWNER,1]))
        balance=lambda t: call(t,calldata("balanceOf(address)",["address"],[OWNER]))
        runs=simulate_block("https://eth.drpc.org",reader.tag,{OWNER:{"balance":hex(2*10**18)}},[fund,approve,balance(USDC),swap,balance(USDC),balance(USDT)])
        assert all(int(runs[i]["status"],16)==1 for i in (0,1,2,4,5)) and int(runs[2]["returnData"],16)==size
        after=[int(runs[i]["returnData"],16) for i in (4,5)]
        if not allowed: assert int(runs[3]["status"],16)==0 and after==[size,0]
        quotes.append(dict(input=size,quoted=q,request=swap,receipt=runs[3],after=after))
    return dict(router=LEGACY,exchange=exchange,approval=approval,enabled=enabled,payer=OWNER,payer_blocked=blocked,payer_allowed=allowed,pool=data,orders=quotes)


def capture(network,extra):
    head=json.loads((ROOT/f"tests/data/first_run_dex_2026-11-26/{network}.json").read_bytes());reader=Reader(network,head);cap=CanonicalCalls(reader)
    observed=[]; helpers=set(); registry=REGISTRY[network]; candidates=set(DIRECT[network]); candidates.update(extra)
    if network=="ethereum":
        for topic in CREATED:
            observed.extend(original_logs(reader,head["block"],topic,23600491,registry))
        candidates.update("0x"+e["topics"][1][-40:] for e in observed)
        for signature,index in (("BladePermitRouterCreated(address,address)",1),("BladeLPTransferCreated(address,address,address)",3)):
            rows=original_logs(reader,head["block"],"0x"+keccak(text=signature).hex(),23600491,registry)
            helpers.update("0x"+e["topics"][index][-40:] for e in rows)
            observed.extend(rows)
    directory=root_check(reader,cap,registry,sorted(candidates|helpers))
    if not directory["complete"]:
        raise ValueError("registered addresses do not reproduce the canonical storage root")
    directory["helpers"]=sorted(set(directory["registered"]) & helpers)
    markets=set(directory["registered"]) - helpers
    pools=[describe(reader,cap,a) for a in sorted(set(DIRECT[network])|markets)]
    data=dict(network=network,height=head["block"],selector=reader.tag,state_root=reader.header["stateRoot"],registry=directory,pools=pools,
              source="https://github.com/sushi-labs/clipper-rfq-subgraph/tree/"+REVISION,registration_events=observed)
    if network=="ethereum":
        data["unsigned_legacy"]=unsigned(reader,cap)
        cove=cap.one(COVE,"CLIPPER_EXCHANGE()",returns=["address"])[0]
        topics=["CoveDeposited(address,address,uint256,uint256)","CoveSwapped(address,address,address,uint256,uint256,bytes32)"]
        events={s:original_logs(reader,head["block"],"0x"+keccak(text=s).hex(),15819271,COVE) for s in topics}
        data["cove"]=dict(address=COVE,lp=cove,from_block=15819271,events=events)
    return data


if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("network",choices=REGISTRY);parser.add_argument("output",type=Path)
    parser.add_argument("--pool",action="append",default=[]);args=parser.parse_args()
    args.output.write_text(json.dumps(capture(args.network,args.pool),separators=(",",":")))
