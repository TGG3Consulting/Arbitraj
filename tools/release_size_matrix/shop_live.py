from pathlib import Path
import json,sys
root=Path(sys.argv[1]);t=Path(__file__).parent
cases=json.loads((t/'shop_live_cases.json').read_text());f=root/'tests/native_routes_shopinx.rs'
s=f.read_text();a=s.index('pub async fn scene_at(');b=s.index('/// Котировка выбора',a);r=s[a:b]
r=r.replace('pub async fn scene_at(dir:&str,block:u64,word:i16,width:i16)','pub async fn live_scene(dir:&str,block:u64,word:i16,other:i16,width:i16)')
r=r.replace('let tag = "SHOPINX-WBNB pcs_v3";','let tag = "0x3079ae2e87dc2e0c75c9b98d65bc622ad10f326c";')
r=r.replace('let pair = super::route_common::v2(&got, "WBNB-USDT v2", PAIR, WBNB, USDT);','let pair = super::route_common::v3(&got,"0x47a90a2d92a8367a91efa1906bfc8c1e05bf10c4",false,1,100,(other-width,other+width),address!("0x47a90a2d92a8367a91efa1906bfc8c1e05bf10c4"),WBNB,USDT);')
assert 'let pair = super::route_common::v3' in r
extra=r.replace("pub async fn live_scene(","pub async fn live_scene_pcs(").replace("0x47a90a2d92a8367a91efa1906bfc8c1e05bf10c4","0x172fcd41e0913e95784454622d1c3724f546f849").replace('",false,1,100,','",true,1,100,')
third=extra.replace('pub async fn live_scene_pcs(','pub async fn live_scene_other(').replace('0x172fcd41e0913e95784454622d1c3724f546f849', next(c['bridge'] for c in cases if c['spacing']==10)).replace('",true,1,100,(other-width,other+width)','",true,10,500,(other-width,other+width)')
f.write_text(s+r+extra+third)
for c in cases:
 b=c['block'];from shop_live_hint import hint; c['hint']=hint(c,t/f'live-shop-{b}.json');dest=root/f'tests/data/shop-live-{b}';dest.mkdir(exist_ok=True);(dest/'responses.json').write_bytes((t/f'live-shop-{b}.json').read_bytes())
(root/'tests/shop_live_cases.json').write_text(json.dumps(cases))
f=root/'tests/native_route_release_matrix.rs';f.write_text(f.read_text()+(t/'shop_live.rs').read_text(encoding='utf-8-sig'))

