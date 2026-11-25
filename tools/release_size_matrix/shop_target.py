from pathlib import Path
import sys
root=Path(sys.argv[1]);tools=Path(__file__).parent; f=root/'tests/native_routes_shopinx.rs';s=f.read_text()
s=s.replace('pub async fn scene_words(width: i16) -> Parts {','pub async fn scene_words(width: i16) -> Parts { scene_at(DIR,BLOCK,WORD,width).await }\npub async fn scene_at(dir:&str,block:u64,word:i16,width:i16) -> Parts {')
s=s.replace('super::route_common::load(DIR)','super::route_common::load(dir)').replace('(WORD - width, WORD + width)','(word - width, word + width)').replace('p.heads.observe(BLOCK)','p.heads.observe(block)')
assert 'pub async fn scene_at' in s;f.write_text(s)
for b in (133910892,133910895):
 target=root/f'tests/data/shopinx-{b}';target.mkdir(exist_ok=True)
 (target/'responses.json').write_bytes((tools/f'shopinx-30327-route-{b}.json').read_bytes())
f=root/'tests/native_route_release_matrix.rs';f.write_text(f.read_text()+(tools/'shop_target.rs').read_text(encoding='utf-8-sig'))
