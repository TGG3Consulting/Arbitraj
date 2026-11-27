"""Public access and already-settled Ethereum Kipseli payments. All API requests are anonymous GET."""
import gzip
import hashlib
import json
from datetime import datetime, timezone
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor

import requests
from eth_abi import decode
from eth_utils import keccak
from record_llama_top_contracts import DATA, rpc, sig


VENUE = '0x71e790dd841c8a9061487cb3e78c288e75ce0b3d'
ROUTER = '0x4ddf368080cd7946db5b459ad591c350158175e1'
WETH = '0xc02aaa39b223fe8d0a0e5c4f27ead9083c756cc2'
USDC = '0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48'
EXECUTED = (
    '0xdbaae6fcb555ad1e128ee64e5468a0757e9836afd6498acb973599a3e92f72e1',
    '0xd1a5aaea99515a8730165796d1df7b63f4a33e0248985f53b391daccbc7cbc4e',
    '0x9ba8e1b8389d2c03d0ec59464aaeb064497d1110891c5fee00899b617117c8a3',
)
def access():
    probes = {
        'kipseli_base_depth':('https://propamm.kipseli.win/v2/price',{'chainId':8453},{}),
        'zeroex_ethereum_price':('https://api.0x.org/swap/allowance-holder/price',
            {'chainId':1,'sellToken':WETH,'buyToken':USDC,'sellAmount':str(10**16)},{'0x-version':'v2'}),
        'zeroex_sources':('https://api.0x.org/sources',{'chainId':1},{'0x-version':'v2'}),
    }
    rows={}
    for tag,(url,params,headers) in probes.items():
        r=requests.get(url,params=params,headers=headers,allow_redirects=False,timeout=20)
        rows[tag]=dict(url=r.url,headers=headers,status=r.status_code,response=r.json(),server_date=r.headers.get('Date'))
    schema=requests.get('https://docs.kipseli.capital/openapi.yaml',timeout=20)
    schema.raise_for_status()
    return dict(at=datetime.now(timezone.utc).isoformat(),probes=rows,kipseli_openapi_sha256=hashlib.sha256(schema.content).hexdigest(),
        docs=['https://docs.kipseli.capital/','https://docs.kipseli.capital/openapi.yaml',
              'https://docs.0x.org/docs/introduction/quickstart/swap-tokens-with-0x-swap-api',
              'https://github.com/gattaca-com/propamm','https://github.com/lambdaclass/propamm-router-contracts/tree/d6e41d3d6f3bf9f8ec78dfea5957f81b7bed467b'])
def canonical(txid):
    receipt=rpc('ethereum','eth_getTransactionReceipt',[txid])
    header=rpc('ethereum','eth_getBlockByNumber',[receipt['blockNumber'],False])
    tx=rpc('ethereum','eth_getTransactionByHash',[txid])
    if tx['blockHash']!=header['hash'] or receipt['blockHash']!=header['hash'] or tx['to']!=ROUTER or receipt['status']!='0x1':
        raise ValueError('Unconfirmed direct transaction')
    if tx['input'][:10] != sig('swapViaVenueV1(address,address,address,uint256,uint256,address,uint256)')[:10]:
        raise ValueError('Different signed call')
    return dict(tx={k:tx[k] for k in ('hash','from','to','value','input','blockNumber','blockHash')},receipt=receipt,
                state_root=header['stateRoot'],timestamp=int(header['timestamp'],16))
def capture():
    source=json.loads((DATA/'ethereum.json').read_bytes())
    at=source['selector']
    def read(address,name,out='address'):
        raw=rpc('ethereum','eth_call',[dict(to=address,data=sig(name)[:10]),at])
        return decode([out],bytes.fromhex(raw[2:]))[0]
    pool=read(VENUE,'propAMM()')
    logic=read(VENUE,'quoteWrapper()')
    safe=read(pool,'wallet()')
    if read(pool,'swapImpl()')!=logic or read(logic,'wallet()')!=safe:
        raise ValueError('Wrong Kipseli payment chain')
    roles={'venue':VENUE,'pool':pool,'logic':logic,'vault':safe,'singleton':read(safe,'masterCopy()')}
    def code_hash(addr):
        data=rpc('ethereum','eth_getCode',[addr,at])
        if data=='0x':raise ValueError('No contract')
        return '0x'+keccak(bytes.fromhex(data[2:])).hex()
    with ThreadPoolExecutor(max_workers=5) as ex:
        codes=dict(zip(roles,ex.map(code_hash,roles.values())))
        payments=list(ex.map(canonical,EXECUTED))
    return dict(chain=1,selector=at,height=source['block'],roles={k:{'address':a,'hash':codes[k]} for k,a in roles.items()},
                listed_pairs=read(VENUE,'getPairs()','(address,address)[]'),ordinary=payments,access=access())


if __name__=='__main__':
    payload=json.dumps(capture(),sort_keys=True,separators=(',',':')).encode()
    (DATA/'kipseli_ethereum.json.gz').write_bytes(gzip.compress(payload,mtime=0))
