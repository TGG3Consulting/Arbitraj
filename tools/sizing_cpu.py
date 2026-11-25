"""Take the production CPU admission path around the same stored DEX router inputs."""
from pathlib import Path
import sys
root=Path(sys.argv[1])
p=root/'tests/native_routes_shopinx.rs';s=p.read_text()
old='routing::run(q.clone(), input).await'
assert s.count(old)==1, s.count(old)
p.write_text(s.replace(old,'arb::venues::bsc_dex::quote::cpu::run(&q.ctx,Box::pin(routing::run(q.clone(), input))).await'))
p=root/'tests/native_route_release_matrix.rs';s=p.read_text()
old='route_common::channel(&parts,"club48","bundle").await;route_common::executor(&parts'
new='for (name,value) in [("cpu.threads_max",80),("cpu.reserve_deal",8),("cpu.reserve_calc",24)]{parts.ctx.settings.set(name,Scope::Global,Some(json!(value)),"recorded profile 2026-11-25").await.unwrap();}'+old
assert old in s;p.write_text(s.replace(old,new))
