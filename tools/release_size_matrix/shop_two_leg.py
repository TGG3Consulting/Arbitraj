import pathlib,re,sys,json,shutil
import native_routes
t=pathlib.Path(sys.argv[1]).resolve()
tools=pathlib.Path(__file__).parent
meta=native_routes.install(t)
assert meta["status"]=="staged",meta
p=t/"tests/native_route_release_matrix.rs"
ctx_source=(t/"src/core/uncapped_recompute_ctx.rs").read_text(encoding="utf-8-sig")
ctx=re.search(r"(?ms)^    let cx = (DirCtx \{.*?^    \});",ctx_source)[1]
for old,new in [("super::reserve::","arb::core::reserve::"),("plan.units_b_per_a","arb::core::settlement_unit::UnitRatio::IDENTITY"),("plan.returns()","true"),("plan.transit_s","transit"),("plan.sigma","sigma")]:
    ctx=ctx.replace(old,new)
mod=(tools/"shop_two_leg.rs").read_text(encoding="utf-8-sig").replace("// SHOP_CTX",ctx)
p.write_text(p.read_text()+mod)
fixture=tools.parents[1]/"tests/data/bingx_minqty_2026-11-15"
for fn,dest in [("book_ticker_SHOPINX-USDT.json","control_shop_book.json"),("symbols_SHOPINX-USDT.json","control_shop_symbol.json")]:
    shutil.copyfile(fixture/fn,t/"tests"/dest)
