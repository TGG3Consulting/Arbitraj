"""Реестр DerpDEX Base. Nonce фабрики растёт только при CREATE2 успешного createPool.
Находим блоки всех изменений; события deployment и этих блоков проверяем в RPC.
Полный trie непустого хранилища доказывает множество пулов и тарифов на блоке.
"""
import json,sys,time,tempfile
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor
import requests
from eth_abi import decode
from eth_utils import keccak
from record_base_slipstream_executor import calldata
from record_derpdex_base import FACTORY,FOLDER
from v3_factory_storage import prove_account,verify_directory

ENDPOINT="https://base.drpc.org"
PORTAL="https://portal.sqd.dev/datasets/base-mainnet/stream"
START=2753388
FEE="0x"+keccak(text="FeeAmountEnabled(uint24,int24)").hex()
POOL="0x"+keccak(text="PoolCreated(address,address,uint24,int24,address)").hex()
def request(method,params):
    if method not in {"eth_call","eth_getBlockByNumber","eth_getBlockByHash","eth_getProof","eth_getLogs","eth_getTransactionCount","eth_chainId"}:
        raise ValueError(method)
    sites=([ "https://mainnet.base.org","https://base.gateway.tenderly.co",ENDPOINT] if method in
           {"eth_getTransactionCount","eth_getLogs"} else [ENDPOINT,"https://base.gateway.tenderly.co","https://mainnet.base.org"])
    for retry in range(7):
        try:
            r=requests.post(sites[min(retry,len(sites)-1)],json={"jsonrpc":"2.0","id":1,"method":method,"params":params},timeout=18)
            if r.ok and "result" in r.json():return r.json()["result"]
            reason=r.text[:200]
        except (requests.RequestException,ValueError) as err:
            reason=str(err)[:200]
        time.sleep(1+retry)
    raise ValueError(reason)
def external_sources():
    from datetime import datetime, timezone
    gt="https://api.geckoterminal.com/api/v2/networks/base/dexes/derpdex-base/pools?page=1"
    listed=requests.get(gt,timeout=12)
    listed.raise_for_status()
    pools=[{"name":r["attributes"]["name"],"pool":r["attributes"]["address"].lower()} for r in listed.json()["data"]]
    sqd={"type":"evm","fromBlock":START,"toBlock":START+2,"fields":{"block":{"number":True,"hash":True},
        "log":{"logIndex":True,"address":True,"topics":True,"data":True,"transactionHash":True}},"logs":[{"address":[FACTORY],"topic0":[FEE]}]}
    logs=[]
    for pause in (0,1,2):
        time.sleep(pause)
        response=requests.post(PORTAL,json=sqd,timeout=14)
        if response.ok:
            logs=[row for line in response.iter_lines() if line for row in json.loads(line).get("logs",[])]
            break
    return {"gecko_url":gt,"gecko_observed":datetime.now(timezone.utc).isoformat(),"indexed":pools,
        "sqd_deployment_url":PORTAL,"sqd_http":response.status_code,"sqd_fee_events":logs}

def collect():
    snap=json.loads((FOLDER/"base.json").read_bytes());tag=snap["selector"];last=snap["block"]
    if int(request("eth_chainId",[]),16)!=8453:raise ValueError("чужая сеть")
    head=request("eth_getBlockByHash",[tag["blockHash"],False])
    proof=request("eth_getProof",[FACTORY,[],tag]);root=prove_account(head,proof,FACTORY)
    lo=int(request("eth_getTransactionCount",[FACTORY,hex(START)]),16);hi=int(proof["nonce"],16)
    changed=[]; intervals=[(START,last,lo,hi)]
    # После отказа общей точки доступа возобновляется только публичный поиск блоков.
    # Каждая итоговая запись независимо проверяется журналом создания и корнем trie.
    local=Path(tempfile.gettempdir())/("derpdex-factory-nonces-"+tag["blockHash"]+".json")
    cache=json.loads(local.read_bytes()) if local.is_file() else {}
    def nonce(split):
        key=str(split[4])
        if key in cache:return cache[key]
        time.sleep(0.25)
        return int(request("eth_getTransactionCount",[FACTORY,hex(split[4])]),16)
    with ThreadPoolExecutor(max_workers=2) as workers:
        while intervals:
            next_intervals=[];split=[]
            for l,h,v,w in intervals:
                if v==w:continue
                if h==l+1:changed.append((h,w-v));continue
                split.append((l,h,v,w,(l+h)//2))
            midcounts=workers.map(nonce,split)
            for (l,h,v,w,m),mv in zip(split,midcounts):
                cache[str(m)]=mv
                if not v<=mv<=w:raise ValueError("немонотонный nonce фабрики")
                next_intervals.extend([(l,m,v,mv),(m,h,mv,w)])
            local.write_text(json.dumps(cache))
            intervals=next_intervals
    def logs(block):
        h=hex(block)
        return request("eth_getLogs",[{"fromBlock":h,"toBlock":h,"address":FACTORY,"topics":[[FEE,POOL]]}])
    tiers={};pools=[];fees=[]
    for block,change in [(START,lo-1),*sorted(changed)]:
        vals=logs(block);before=len(pools)
        for x in vals:
            ts=x["topics"]
            if ts[0]==FEE:
                f=int(ts[1],16);sp=decode(["int24"],bytes.fromhex(ts[2][2:]))[0];tiers[str(f)]=sp;fees.append(x)
            elif ts[0]==POOL:
                sp,p=decode(["int24","address"],bytes.fromhex(x["data"][2:]));fee=int(ts[3],16)
                pools.append({"pool":p,"tokens":["0x"+t[-40:] for t in ts[1:3]],"fee":fee,"spacing":sp,
                    "block":block,"hash":x["blockHash"],"tx":x["transactionHash"],"log":int(x["logIndex"],16)})
        if len(pools)-before!=change:raise ValueError("не все CREATE2 фабрики подтверждены событием")
    source=requests.get("https://sourcify.dev/server/v2/contract/8453/"+FACTORY,params={"fields":"storageLayout"},timeout=15).json()["storageLayout"]
    layout={v["label"]:int(v["slot"]) for v in source["storage"]}
    owner=request("eth_call",[{"to":FACTORY,"data":calldata("owner()")},tag])
    data={**external_sources(),"chain_id":8453,"factory":FACTORY,"from":START,"to":last,"selector":tag,
            "state_root":head["stateRoot"],"account_proof":proof["accountProof"],"storage_root":"0x"+root.hex(),
            "nonce":hi,"owner":owner,"layout":layout,"storage_slots":2*len(pools)+len(tiers)+bool(int(owner,16)),"fee_events":fees,
            "tiers":tiers,"pools":pools}
    verify_directory(data,FACTORY)
    return data
if __name__=="__main__":
    if sys.argv[1]=="--verify-file":
        verify_directory(json.loads(Path(sys.argv[2]).read_bytes()),FACTORY)
    else:
        Path(sys.argv[1]).write_bytes((json.dumps(collect(),indent=2)+"\n").encode())
