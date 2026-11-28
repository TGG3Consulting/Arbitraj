"""Measure bopAMM/USDC plus the real Ethereum Uniswap V3 USDC/USDT pool as one isolated transaction."""
import gzip
import json
import sys
from pathlib import Path

from eth_abi import decode,encode
from eth_utils import keccak
from fork_fee_rpc import Reader
from record_bopamm_wallet import AMM,PAMM,REGISTRY,ROUTER,USDT,USDC,WETH,DATA,ORDER,tx,maker_fills,create_address


# Funded historical EOA in the Tokenlon payment archive; existing ETH and USDT are checked at both parents.
PAYER="0x8cbcad0de3a585450825accc9f61c346352fe889"
FACTORY="0x1f98431c8ad98523631ae4a59f267346ea31f984"
UNI="0xe592427a0aece92de3edee1f18e0157c05861564"
FEE=100
SWAP="0x"+keccak(text="Swap(address,address,int256,int256,uint160,uint128,int24)").hex()

def node_tx(to,name,types=(),values=(),value=0):
    return {**tx(to,name,types,values,value),"from":PAYER}

def read_balance(t):
    return node_tx(t,"balanceOf(address)",["address"],[PAYER])

def run(reader,base,probe,frame,cache):
    tag={"blockHash":frame["parent"],"requireCanonical":True}
    height=frame["target"]
    slot_time=1606824023+frame["slot"]*12
    if frame["parent"] not in cache:
        h,nonce,eth,usdt,pool,next_block=reader.many([("eth_getBlockByHash",[frame["parent"],False]),
            ("eth_getTransactionCount",[PAYER,tag]),("eth_getBalance",[PAYER,tag]),("eth_call",[read_balance(USDT),tag]),
            ("eth_call",[node_tx(FACTORY,"getPool(address,address,uint24)",["address","address","uint24"],[USDC,USDT,FEE]),tag]),
            ("eth_getBlockByNumber",[hex(height),False])])
        assert int(h["number"],16)+1==height and next_block["parentHash"]==frame["parent"]
        assert int(eth,16)>=5*10**18 and int(usdt,16)>=10_000*10**6
        pool="0x"+pool[-40:];assert int(pool,16)!=0
        base_addr=create_address(PAYER,int(nonce,16))
        probe_addr=create_address(PAYER,int(nonce,16)+1)
        assert reader.many([("eth_getCode",[base_addr,tag]),("eth_getCode",[probe_addr,tag])])==["0x","0x"]
        receipt=reader.one("eth_getTransactionReceipt",[next_block["transactions"][len(next_block["transactions"])//2]])
        assert receipt["blockHash"]==next_block["hash"]
        cache[frame["parent"]]=(base_addr,probe_addr,pool,int(receipt["effectiveGasPrice"],16),receipt["transactionHash"],int(eth,16),int(usdt,16))
    base_addr,probe_addr,pool,gas_price,ref,eth,usdt=cache[frame["parent"]]
    create1={"from":PAYER,"data":"0x"+base["creation"]+encode(["address"]*3,[AMM,PAMM,USDC]).hex(),"gas":"0x4c4b40"}
    create2={"from":PAYER,"data":"0x"+probe["creation"]+encode(["address"],[base_addr]).hex(),"gas":"0x4c4b40"}
    patch={REGISTRY:{"stateDiff":frame["stateDiff"]}}
    def measure(source,target,size,bound,compound):
        spender=probe_addr if compound else ROUTER
        grants=[node_tx(source,"approve(address,uint256)",["address","uint256"],[spender,0]),
                node_tx(source,"approve(address,uint256)",["address","uint256"],[spender,size])]
        if source==WETH:
            grants.insert(0,node_tx(WETH,"deposit()",value=size))
        reads=[read_balance(t) for t in [source,target,USDC]]
        leg=keccak(encode(["address","uint256","uint256"],[source,size,height]))
        o=(leg,bytes.fromhex(tag["blockHash"][2:]),height,slot_time,source,target,size,bound)
        swap=(node_tx(probe_addr,"execute("+ORDER+",uint24)",[ORDER,"uint24"],[o,FEE]) if compound else
            node_tx(ROUTER,"swapViaVenueV1(address,address,address,uint256,uint256,address,uint256)",["address","address","address","uint256","uint256","address","uint256"],[AMM,source,target,size,bound,PAYER,slot_time]))
        calls=[create1,create2,*grants,*reads,swap,*reads]
        req={"blockStateCalls":[{"blockOverrides":{"time":hex(slot_time)},"stateOverrides":patch,"calls":calls}]}
        h,=reader.one("eth_simulateV1",[req,tag]);assert h["parentHash"]==frame["parent"] and int(h["number"],16)==height
        v=h["calls"];assert len(v)==len(calls)
        assert all(c["status"]=="0x1" for c in v[:-4]+v[-3:])
        before=[int(x["returnData"],16) for x in v[-7:-4]]
        after=[int(x["returnData"],16) for x in v[-3:]]
        mid=v[-4]
        if mid["status"]=="0x0":
            assert before==after
            return {"outcome":"reverted","reason":str(mid.get("error",{}).get("message",""))[:130]}
        spent=before[0]-after[0];received=after[1]-before[1]
        assert spent==size and received>=bound and before[2]==after[2]
        if compound:
            assert decode(["uint256"],bytes.fromhex(mid["returnData"][2:]))==(received,)
            b_in,b_out=(USDC,WETH) if source==USDT else (WETH,USDC)
            pair,=[x for x in mid["logs"] if x["address"].lower()==pool and x["topics"][:1]==[SWAP]]
            a0,a1,*_=decode(["int256","int256","uint160","uint128","int24"],bytes.fromhex(pair["data"][2:]))
            inter=-a0 if source==USDT else a0
            assert inter>0 and (a1==size if source==USDT else -a1==received)
            fills=maker_fills(mid["logs"],b_in,b_out,probe_addr,inter if source==USDT else size,received if source==USDT else inter)
            venue=AMM
        else:
            amount,venue=decode(["uint256","address"],bytes.fromhex(mid["returnData"][2:]))
            assert amount==received
            fills=[]
        return {"outcome":"paid","spent":str(spent),"received":str(received),"gas":int(mid["gasUsed"],16),
                "approval_gas":int(v[1+len(grants)]["gasUsed"],16),"venue":venue,"maker_fills":fills}
    rows=[]
    for source,target,size in [(WETH,USDT,5*10**17),(USDT,WETH,1000*10**6),(WETH,USDT,5*10**18),(USDT,WETH,10_000*10**6)]:
        combo=measure(source,target,size,1,True);direct=measure(source,target,size,1,False)
        if combo['outcome']=='paid':
            exact=int(combo['received'])
            assert measure(source,target,size,exact+1,True)['outcome']=='reverted'
            # The amount credited by the same canonical atomic call is protected by this minimum.
            locked=measure(source,target,size,exact,True)
            assert (locked['spent'],locked['received'])==(combo['spent'],combo['received'])
            combo=locked
        rows.append({"input":source,"output":target,"amount":str(size),"compound":combo,"direct":direct})
    return {"height":height,"slot":frame["slot"],"parent":frame["parent"],"stable_pool":pool,"stable_fee":FEE,
            "gas_price_wei":str(gas_price),"gas_reference":ref,"holder":PAYER,"initial_eth":str(eth),"initial_usdt":str(usdt),"cases":rows}

if __name__=="__main__":
    base,probe=[json.loads(Path(p).read_bytes()) for p in sys.argv[1:3]]
    reader=Reader("ethereum",json.loads((DATA/"ethereum.json").read_bytes()))
    archive=json.loads(gzip.decompress((DATA/"bopamm_ethereum_frames.json.gz").read_bytes()))
    published=json.loads(gzip.decompress((DATA/"bopamm_ethereum_published.json.gz").read_bytes()))
    cache={};results=[]
    for feed in published:
        builder=feed["builder"]
        for published_frame in feed["frames"]:
            frame=next(v[builder] for v in archive if v[builder].get("same_live_parent") and
                v[builder].get("parent")==published_frame["parent"] and v[builder].get("stateDiff"))
            assert frame["registry"]==REGISTRY and frame["target"]==published_frame["target"] and frame["other_patched_accounts"]==0
            results.append({"builder":builder,**run(reader,base,probe,frame,cache)})
    Path(sys.argv[3]).write_text(json.dumps(results,separators=(",",":")))
