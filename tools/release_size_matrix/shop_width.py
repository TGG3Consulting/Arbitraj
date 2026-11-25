from pathlib import Path
import sys
root=Path(sys.argv[1]); tools=Path(__file__).parent
f=root/'tests/native_routes_shopinx.rs'; t=f.read_text()
t=t.replace('pub async fn scene() -> Parts {','pub async fn scene() -> Parts { scene_words(2).await }\npub async fn scene_words(width: i16) -> Parts {').replace('(WORD - 2, WORD + 2)','(WORD - width, WORD + width)')
assert 'scene_words(width:' in t
f.write_text(t)
f=root/'tests/native_route_release_matrix.rs'; t=f.read_text()
t=t.replace('    book: FrozenBook,','    book: FrozenBook,\n    reverse: bool,')
t=t.replace('if sell {\n            shopinx::full(&self.parts.quoting(), Side::Sell, qty)', 'if sell != self.reverse {\n            shopinx::full(&self.parts.quoting(), if sell {Side::Sell} else {Side::Buy}, qty)')
t=t.replace('if sell { return Ok(avg); }','if sell != self.reverse { return Ok(avg); }')
t=t.replace('deepest(&self.book.book,Side::Buy,qty)','deepest(&self.book.book,if sell {Side::Sell} else {Side::Buy},qty)')
t=t.replace('Ok(vec![Cost::new(CostKind::WithdrawFee', 'if self.reverse {return Ok(vec![Cost::new(CostKind::TransferGas,num("0.000021"),"BNB",false)]);}\n        Ok(vec![Cost::new(CostKind::WithdrawFee')
t=t.replace('async fn ret(&self, _: &DirCtx, _: Decimal) -> Result<Vec<Cost>,Fail> {','async fn ret(&self, _: &DirCtx, _: Decimal) -> Result<Vec<Cost>,Fail> {\n        if self.reverse {return Ok(vec![Cost::new(CostKind::UsdtReturn,num("0.15"),"USDT",false)]);}')
t=t.replace('TwoLeg{parts:&parts,book:', 'TwoLeg{parts:&parts,reverse:false,book:')
f.write_text(t+(tools/'shop_width.rs').read_text(encoding='utf-8-sig'))
