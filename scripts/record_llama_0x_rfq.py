"""Одна RFQ нога 0x и два чужих обмена из канонической транзакции Ethereum. Только чтение."""
import gzip
import json
from zeroex_rfq_format import PATTERN, REGISTRY

from record_llama_top_contracts import DATA, rpc, sig

TRADE='0x072567a712fd09969b916f168e72cf458cbb2697843996b220d94c2fc26a133d'
REGISTER='0x6050acc28c4cf57fa5576be05fce3940f16a35b887cacc9c9fffefffa64f17a2'

def receipt(txhash):
    r=rpc('ethereum','eth_getTransactionReceipt',[txhash])
    h=rpc('ethereum','eth_getBlockByNumber',[r['blockNumber'],False])
    if h['hash']!=r['blockHash'] or r['status']!='0x1':raise ValueError('Missing canonical transaction')
    return r

def record():
    r, birth=receipt(TRADE),receipt(REGISTER)
    tx=rpc('ethereum','eth_getTransactionByHash',[TRADE])
    if tx['blockHash']!=r['blockHash']:raise ValueError('Reorg')
    t=sig('Transfer(address,address,uint256)')
    selections=[l for l in birth['logs'] if l['address']==REGISTRY and
                len(l['topics'])==4 and l['topics'][0]==t and int(l['topics'][3],16)==2]
    markers=[l for l in r['logs'] if not l['topics'] and len(l['data'])==98 and int(l['data'][2:34],16)]
    actions=list(PATTERN.finditer(tx['input']))
    if len(markers)!=1 or len(actions)!=1:raise ValueError('Ambiguous marker/action')
    m, action=markers[0], actions[0]
    creation=[l for l in selections if '0x'+l['topics'][2][-40:]==m['address']]
    if len(creation)!=1:raise ValueError('Wrong Settler')
    token='0x'+(action[1] or action[3]); amount=int(m['data'][-32:],16)
    if not 0<amount<=int(action[2] or action[4],16):raise ValueError('RFQ amount incompatible with order')
    legs=[l for l in r['logs'] if len(l['topics'])==3 and l['topics'][0]==t]
    output=[l for l in legs if l['address']==token and int(l['data'],16)==amount and '0x'+l['topics'][2][-40:]==m['address']]
    if len(output)!=1:raise ValueError('Ambiguous maker payment')
    maker='0x'+output[0]['topics'][1][-40:]
    funds=[l for l in legs if l['address']!=token and '0x'+l['topics'][1][-40:]==m['address'] and '0x'+l['topics'][2][-40:]==maker]
    if len(funds)!=1 or int(funds[0]['data'],16)==0:raise ValueError('Unproven maker funding')
    return dict(source='dexs/0x-rfq.ts at be9967c3f0d3ccd06c871bab0d16beb3304de719',network='ethereum',
                registration=creation[0],registry=REGISTRY,settler=m['address'],marker=m,
                transaction={k:tx[k] for k in ('hash','blockNumber','blockHash','input','to','from')},
                receipt=r,maker=maker,maker_input=funds[0],maker_output=output[0],
                api='https://docs.0x.org/docs/introduction/quickstart/swap-tokens-with-0x-swap-api')
if __name__=='__main__':
    (DATA/'llama_top_0x_rfq.json.gz').write_bytes(gzip.compress(json.dumps(record(),separators=(',',':')).encode(),mtime=0))
