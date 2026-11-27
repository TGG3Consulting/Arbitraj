"""Закрепляет реестр и события рынков из двадцатки DefiLlama обычными каноническими чтениями."""
import gzip
import json
import threading
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import requests
import websocket
from eth_abi import decode, encode
from eth_utils import keccak

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'tests/data/first_run_dex_2026-11-26'
LOCAL = threading.local()
SWAP = 'Swap(address,address,int256,int256,uint160,uint128,int24)'
DEPRECATED = '0x9592cd9b267748cbfbde90ac9f7df3c437a6d51b'

def sig(name):
    return '0x' + keccak(text=name).hex()

def rpc(network, method, args):
    if method not in ('eth_chainId', 'eth_getBlockByNumber', 'eth_getLogs', 'eth_call', 'eth_getCode','eth_getTransactionReceipt','eth_getTransactionByHash'):
        raise ValueError(method)
    if (network == 'ethereum' and method in ('eth_call','eth_getCode','eth_getTransactionReceipt')) or (network == 'base' and (method == 'eth_getCode' or (method=='eth_getLogs' and args[0]['fromBlock']==args[0]['toBlock']) or (method=='eth_getBlockByNumber' and int(args[0],16)<51000000))):
        session=LOCAL.__dict__.setdefault('http',requests.Session())
        last=None
        providers=('https://eth.drpc.org','https://ethereum-public.nodies.app') if network=='ethereum' else ('https://base.gateway.tenderly.co','https://base.drpc.org','https://base-public.nodies.app')
        for url in providers:
            try:
                response=session.post(url,json=dict(jsonrpc='2.0',id=1,method=method,params=args),timeout=20)
                response.raise_for_status()
                reply=response.json()
                if 'result' not in reply:raise ValueError(str(reply.get('error'))[:180])
                return reply['result']
            except (requests.RequestException,ValueError) as exc:
                last=exc
        raise RuntimeError(f'{method} {args[0]}') from last
    url = f'wss://{network}-rpc.publicnode.com'
    for trial in range(3):
        sockets = LOCAL.__dict__.setdefault('sockets', {})
        try:
            if network not in sockets:
                sockets[network] = websocket.create_connection(url, timeout=30, suppress_origin=True)
            ws = sockets[network]
            ws.send(json.dumps(dict(id=1, jsonrpc='2.0', method=method, params=args)))
            reply = json.loads(ws.recv())
            if 'result' not in reply:
                raise ValueError(str(reply.get('error'))[:180])
            return reply['result']
        except (ValueError, websocket.WebSocketException, OSError):
            if network in sockets:
                sockets.pop(network).close()
            if trial == 2:
                raise

def capture(network):
    source = json.loads((DATA/f'{network}.json').read_bytes())
    h, at = source['block'], source['selector']
    def block(n):
        return rpc(network, 'eth_getBlockByNumber', [hex(n), False])
    anchor = block(h)
    if anchor['hash'] != at['blockHash'] or int(rpc(network,'eth_chainId',[]),16) != source['chain_id']:
        raise ValueError('Wrong canonical anchor')
    def call(addr, name, types=(), args=(), returned='uint256'):
        data = sig(name)[:10] + encode(types,args).hex()
        raw = rpc(network, 'eth_call', [dict(to=addr, data=data), at])
        return decode([returned], bytes.fromhex(raw[2:]))[0]
    def events(emitters, topic, lo, hi):
        return [item for start in range(lo,hi+1,8192) for item in rpc(network,'eth_getLogs',
            [dict(address=emitters,topics=[topic],fromBlock=hex(start),toBlock=hex(min(hi,start+8191)))])]
    def sample(logs):
        if not logs:
            return None
        v=logs[-1]
        if block(int(v['blockNumber'],16))['hash'] != v['blockHash']:
            raise ValueError('Orphaned log')
        return {k:v[k] for k in ('address','topics','data','transactionHash','blockHash','blockNumber','logIndex')}
    def window():
        upper, step = h, 8192
        cutoff = int(anchor['timestamp'],16)-86400
        lower=max(0,h-step)
        while lower and int(block(lower)['timestamp'],16) >= cutoff:
            step*=2
            lower=max(0,h-step)
        while lower < upper:
            m=(lower+upper)//2
            if int(block(m)['timestamp'],16) < cutoff:
                lower=m+1
            else:
                upper=m
        return lower
    result=dict(network=network,chain=source['chain_id'],height=h,selector=at,state_root=anchor['stateRoot'])
    lo=window()
    result['window']=[lo,h]
    resolver = {'base':'0x160ffc75904515f38c9b7ed488e1f5a43ce71eba','ethereum':'0xe8a07a32489bd9d5a00f01a55749cf5cb854fd13'}[network]
    factory='0x91716c4eda1fb55e84bf8b4c7085f84285c19085'
    pools=call(resolver,'getAllPools()',returned='(address,address,address,uint256)[]')
    n=call(factory,'totalDexes()')
    with ThreadPoolExecutor(5) as ex:
        indexed=list(ex.map(lambda i:call(factory,'getDexAddress(uint256)',['uint256'],[i],returned='address'),range(1,n+1)))
    result['fluid']=dict(resolver=resolver,factory=factory,resolver_pools=pools,factory_pools=indexed)
    if set(indexed)!={p[0] for p in pools}:
        raise ValueError('Resolver and factory differ')
    # Точечное сопоставление реальной пары и фабрики. Котировка и допуск к отправке остаются отдельными доказательствами.
    profiles={x['id']:x['settings'] for x in json.loads((ROOT/'config/first_run_dex.json').read_bytes()) if x['network']==network}
    catalogs=json.loads((ROOT.parent/'outputs'/f'{network}-integration-2026-11-26'/'sources'/'sample-pools.json').read_bytes())
    ids=(['uniswap-v3-base','pancakeswap-v3-base','sushiswap-v3-base','aerodrome-slipstream','aerodrome-slipstream-2','aerodrome-slipstream-3','aero-slipstream-base'] if network=='base' else ['uniswap_v3','aero-slipstream-ethereum'])
    def cl_market(id):
        factory=profiles[id]['dex.factory'].lower()
        if id.startswith('aero-slipstream-'):
            pool=json.loads((DATA/f'metadex_{network}.json').read_bytes())['pools'][0]['pool']
        else:pool=catalogs[id]['samples'][0]['address']
        spacing='slipstream' in id
        tokens=[call(pool,f'token{k}()',returned='address') for k in (0,1)]
        typ='int24' if spacing else 'uint24'
        tier=call(pool,'tickSpacing()' if spacing else 'fee()',returned=typ)
        if call(pool,'factory()',returned='address')!=factory or call(factory,f'getPool(address,address,{typ})',['address','address',typ],[*tokens,tier],returned='address')!=pool:
            raise ValueError(id)
        topic=sig('Swap(address,address,int256,int256,uint160,uint128,int24,uint128,uint128)' if id.startswith('pancake') else SWAP)
        logs=events(pool,topic,h-1023,h)
        return dict(profile=id,factory=factory,pool=pool,tokens=tokens,parameter=tier,parameter_type=typ,swap_topic=topic,window=[h-1023,h],count=len(logs),sample=sample(logs))
    with ThreadPoolExecutor(3) as ex:result['registered_cl']=list(ex.map(cl_market,ids))
    manager=profiles['uniswap-v4-'+network]['dex.pool_manager'].lower()
    topic=sig('Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)')
    logs=events(manager,topic,h-15,h)
    result['v4']=dict(manager=manager,topic=topic,count=len(logs),sample=sample(logs),window=[h-15,h])
    if network=='base':
        factory_hanji='0xc7264db7c78dd418632b73a415595c7930a9eea4'
        born=sig('OnchainCLOBCreated(address,address,address,address,bool,uint256,uint256,address,address,address,bool,uint64,uint64,uint64,uint64)')
        registry=json.loads((DATA/'hanji_base.json').read_bytes())['markets']
        def book_birth(row):
            book=row['registry']['orderbookAddress']
            lower,upper=37860153,h
            while lower<upper:
                mid=(lower+upper)//2
                if rpc(network,'eth_getCode',[book,hex(mid)])!='0x':upper=mid
                else:lower=mid+1
            created=events(factory_hanji,born,lower,lower)
            exact=[log for log in created if '0x'+log['data'][26:66]==book]
            return dict(book=book,birth=lower,creation=sample(exact))
        with ThreadPoolExecutor(4) as ex:
            result['hanji']=dict(factory=factory_hanji,topic=born,books=list(ex.map(book_birth,registry)))
        n=call(DEPRECATED,'allPoolsLength()')
        def market(i):
            pool=call(DEPRECATED,'allPools(uint256)',['uint256'],[i],returned='address')
            tokens=[call(pool,f'token{k}()',returned='address') for k in (0,1)]
            tick=call(pool,'tickSpacing()',returned='int24')
            registry=call(DEPRECATED,'getPool(address,address,int24)',['address','address','int24'],[*tokens,tick],returned='address')
            if registry != pool or call(pool,'factory()',returned='address')!=DEPRECATED:raise ValueError(pool)
            balances=[call(t,'balanceOf(address)',['address'],[pool]) for t in tokens]
            return dict(index=i,pool=pool,tokens=tokens,tick_spacing=tick,liquidity=str(call(pool,'liquidity()')),balances=list(map(str,balances)))
        with ThreadPoolExecutor(5) as ex:
            old=list(ex.map(market,range(n)))
        addresses=[p['pool'] for p in old]
        recent=[log for i in range(0,len(addresses),8) for log in events(addresses[i:i+8],sig(SWAP),lo,h)]
        recent.sort(key=lambda log:(int(log['blockNumber'],16),int(log['logIndex'],16)))
        result['early_slipstream']=dict(factory=DEPRECATED,pools=old,last24h_swaps=len(recent),sample=sample(recent))
    addresses={'ethereum':{'kipseli':('0x054f0377e07d2f460151f935dffc4d880017e63a','0xa441699ef46589494a7167a03b4dc78a5adfb1dbf33323acd42fbdd30a27c5b0')},'base':{}}[network]
    for ident,(address,topic) in addresses.items():
        logs=events(address,topic,lo,h)
        entry=dict(address=address,topic=topic,count=len(logs),sample=sample(logs))
        if ident == 'kipseli' and entry['sample']:
            event=entry['sample']
            receipt=rpc(network,'eth_getTransactionReceipt',[event['transactionHash']])
            if receipt['blockHash']!=event['blockHash'] or receipt['status']!='0x1':raise ValueError('Orphaned payment')
            safe=call(address,'wallet()',returned='address')
            token_in,token_out,paid,got,_,recipient=decode(
                ['address','address','uint256','uint256','uint256','address'],bytes.fromhex(event['data'][2:])[:192])
            legs=[log for log in receipt['logs'] if len(log['topics'])==3 and log['topics'][0]==sig('Transfer(address,address,uint256)')]
            def moved(token,src,dest,amount):
                return [log for log in legs if log['address']==token and '0x'+log['topics'][1][-40:]==src and
                        '0x'+log['topics'][2][-40:]==dest and int(log['data'],16)==amount]
            cash_in=moved(token_in,address,safe,paid)
            cash_out=moved(token_out,safe,recipient,got)
            if len(cash_in)!=1 or len(cash_out)!=1:raise ValueError('Ambiguous Kipseli cash flows')
            entry.update(wallet=safe,token_in=token_in,token_out=token_out,amount_in=str(paid),amount_out=str(got),
                         recipient=recipient,transfers=cash_in+cash_out)
        result[ident]=entry
    if block(h)['hash']!=at['blockHash']:
        raise ValueError('Anchor changed')
    return result

if __name__ == '__main__':
    with ThreadPoolExecutor(2) as workers:
        records = list(workers.map(capture,['base','ethereum']))
    (DATA/'llama_top_contracts.json.gz').write_bytes(gzip.compress(json.dumps(records,separators=(',',':')).encode(),mtime=0))
