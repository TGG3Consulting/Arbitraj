"""Captures one spent Base Kipseli authorization and the named cash movements without submitting an order."""
import gzip
import json
import requests
from eth_abi import decode, encode
from eth_utils import keccak
from record_llama_top_contracts import DATA,sig

HASH='0x6d07703b58449f0412748e3a9506fa92363575d8078fc6f9eee2cf32d63a0e5a'
ROUTER='0x71c2ed90cc288229be59f26b8b3eef3c07d7ab99'
SWAP='swap(address,uint256,address,uint256,uint256,bytes)'
TOPIC='0x9734819749a91fc3be03ea83205f924ee08479bd3f0da48efc91d94d050cac1e'
def capture():
    with requests.Session() as s:
        def rpc(method,args):
            if method not in ('eth_chainId','eth_getTransactionByHash','eth_getTransactionReceipt','eth_getBlockByNumber','eth_call'):
                raise ValueError(method)
            r=s.post('https://base.gateway.tenderly.co',json=dict(id=1,jsonrpc='2.0',method=method,params=args),timeout=20)
            r.raise_for_status()
            return r.json()['result']
        tx=rpc('eth_getTransactionByHash',[HASH]);r=rpc('eth_getTransactionReceipt',[HASH])
        h=rpc('eth_getBlockByNumber',[tx['blockNumber'],False])
        if rpc('eth_chainId',[])!='0x2105' or h['hash']!=tx['blockHash'] or r['blockHash']!=h['hash'] or r['status']!='0x1':
            raise ValueError('Unconfirmed payment')
        at={'blockHash':h['hash'],'requireCanonical':True}
        current=decode(['address'],bytes.fromhex(rpc('eth_call',[{'to':ROUTER,'data':sig('swapImpl()')[:10]},at])[2:]))[0]
    raw=bytes.fromhex(tx['input'][2:]);sel=keccak(text=SWAP)[:4]
    if raw.count(sel)!=1:raise ValueError('Ambiguous action')
    types=['address','uint256','address','uint256','uint256','bytes']
    a=decode(types,raw.split(sel)[1])
    proof=decode(['uint256','uint256','address','bytes'],a[5])
    fee,rate,user,authorization=proof
    if len(authorization)!=65 or authorization[-1] not in (27,28):raise ValueError('Invalid historical signature encoding')
    events=[l for l in r['logs'] if l['address']==ROUTER and l['topics']==[TOPIC]]
    if len(events)!=1:raise ValueError('Ambiguous router event')
    i,o,paid,net,time=decode(['address','address','uint256','uint256','uint256'],bytes.fromhex(events[0]['data'][2:]))
    if (i,o,paid)!=(a[0],a[2],a[1]) or net<a[3] or time!=int(h['timestamp'],16):raise ValueError('Amounts or time differ')
    legs=[(l['address'],'0x'+l['topics'][1][-40:],'0x'+l['topics'][2][-40:],int(l['data'],16)) for l in r['logs']
          if len(l['topics'])==3 and l['topics'][0]==sig('Transfer(address,address,uint256)')]
    outgoing=[l for l in legs if (l[0],l[1],l[3])==(i,current,paid)]
    credits=[l for l in legs if (l[0],l[2],l[3])==(o,user,net)]
    if len(outgoing)!=1 or len(credits)!=1:raise ValueError('Ambiguous forward or payout')
    cash=credits[0][1];forward=outgoing[0][2]
    if legs.count((i,user,current,paid))!=1 or legs.count((i,forward,cash,paid))!=1:raise ValueError('Wrong input path')
    return dict(chain=8453,selector=at,state_root=h['stateRoot'],timestamp=time,router=ROUTER,swap_impl=current,
                intermediary=forward,cash=cash,user=user,request_timestamp=a[4],fee=str(fee),origin_rate=str(rate),
                paid=str(paid),received=str(net),minimum=str(a[3]),token_in=i,token_out=o,
                transaction={k:tx[k] for k in ('hash','from','to','blockNumber','blockHash','input')},receipt=r,
                settled_action='0x'+(sel+encode(types,a)).hex())
if __name__=='__main__':
    (DATA/'kipseli_base_settled.json.gz').write_bytes(gzip.compress(json.dumps(capture(),separators=(',',':')).encode(),mtime=0))
