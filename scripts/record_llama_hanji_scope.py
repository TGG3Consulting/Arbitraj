"""Сверяет опубликованные Hanji книги с эмитентами реального агрессивного исполнения из адаптера DefiLlama."""
import gzip
import json

from eth_abi import decode
from record_llama_top_contracts import DATA, rpc, sig

def events(contracts,topic,lo,hi):
    return rpc('base','eth_getLogs',[dict(address=contracts,topics=[topic],fromBlock=hex(lo),toBlock=hex(hi))])

def record():
    directory=json.loads(gzip.decompress((DATA/'llama_top_contracts.json.gz').read_bytes()))[0]
    config=directory['hanji'];h=directory['height']
    sources=config['books']
    if len(sources)!=8:raise ValueError('Unexpected API directory')
    registration=[]
    for n in (37860154,37860159,37860166):
        logs=events(config['factory'],config['topic'],n,n)
        for l in logs:
            head=rpc('base','eth_getBlockByNumber',[hex(n),False])
            if head['hash']!=l['blockHash']:raise ValueError('Registration reorg')
            registration.append(l)
    physical=['0x'+l['data'][26:66] for l in registration]
    api=[r['book'] for r in sources]
    if set(physical)&set(api) or len(set(physical))!=3:raise ValueError('Ambiguous control books')
    placed=sig('OrderPlaced(address,address,uint64,bool,uint128,uint72,uint128,uint128,uint128,uint128,uint128,bool,bool)')
    stream=events(physical,placed,h-8191,h);public=events(api,placed,h-8191,h)
    schema=['uint64','uint128','uint72','uint128','uint128','uint128','uint128','uint128','bool','bool']
    def traded(log):
        values=decode(schema,bytes.fromhex(log['data'][2:]))
        return values[5]>0 and values[6]>0
    fills=[log for log in stream if traded(log)]
    tx='0x6cc86d219b2520f3c3a3961fa990ce6b9a7f4f0a83015028e98238a6afd2a248'
    r=rpc('base','eth_getTransactionReceipt',[tx])
    head=rpc('base','eth_getBlockByNumber',[r['blockNumber'],False])
    if r['status']!='0x1' or r['blockHash']!=head['hash'] or not any(log['transactionHash']==tx for log in fills):
        raise ValueError('Wrong execution sample')
    trades=[log for log in r['logs'] if len(log['topics'])==3 and log['topics'][0]==sig('Transfer(address,address,uint256)')]
    between=[log for log in trades if {('0x'+t[-40:]) for t in log['topics'][1:]} & set(physical)
             and {('0x'+t[-40:]) for t in log['topics'][1:]} & set(api)]
    if len(between)!=2 or between[0]['address']==between[1]['address']:raise ValueError('Unproven shared payment')
    return dict(factory=config['factory'],registration=registration,api=api,physical=physical,
                selector=directory['selector'],window=[h-8191,h],physical_placed=len(stream),physical_filled=len(fills),
                api_placed=len(public),api_filled=sum(map(traded,public)),receipt=r,cross_payments=between)
if __name__=='__main__':
    (DATA/'llama_top_hanji.json.gz').write_bytes(gzip.compress(json.dumps(record(),separators=(',',':')).encode(),mtime=0))
